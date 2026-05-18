#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io;
use std::mem;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use virtiofsd::filesystem::ROOT_ID;
use virtiofsd::filesystem::{
    Context, DirEntry, DirectoryIterator, Entry, Extensions, FileSystem, FsOptions, GetxattrReply,
    ListxattrReply, OpenOptions, SerializableFileSystem, SetattrValid, SetxattrFlags,
    ZeroCopyReader, ZeroCopyWriter,
};
use virtiofsd::fuse;
use virtiofsd::soft_idmap::{GuestGid, GuestUid};

mod host_ops;
mod manifest;
mod namespace;
mod server;
mod state;
pub use manifest::validate_manifest_json_shape;
pub use server::{run_cli, serve_vhost_user_fs, ServeConfig};

pub(crate) use namespace::{HostIdentity, MountRuntime, Namespace, Node, NodeKind};

use host_ops::*;
#[cfg(any(test, feature = "fuzzing"))]
use manifest::FilterSpec;
#[cfg(test)]
use manifest::{AccessMode, FilterAction};
use manifest::{Manifest, MountKind};
#[cfg(any(test, feature = "fuzzing"))]
use manifest::{MetadataPolicy, MetadataSpec, MountSpec, SourceClass, SyntheticSpec};
use state::{FileHandle, HandleTable, LockKey, LockTable};

const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TAG: &str = "agentvm";
pub const DEFAULT_THREAD_POOL_SIZE: usize = 1;
const ATTR_TTL: Duration = Duration::from_secs(1);
const ENTRY_TTL: Duration = Duration::from_secs(1);
const SETLKW_MAX_WAIT: Duration = Duration::from_millis(250);
const SETLKW_RETRY_DELAY: Duration = Duration::from_millis(5);

#[derive(Clone)]
struct ComposedFs {
    namespace: Arc<RwLock<Namespace>>,
    handles: Arc<RwLock<HandleTable>>,
    locks: Arc<Mutex<LockTable>>,
}

impl ComposedFs {
    fn new(namespace: Namespace) -> Self {
        Self {
            namespace: Arc::new(RwLock::new(namespace)),
            handles: Arc::new(RwLock::new(HandleTable {
                next_handle: 1,
                files: HashMap::new(),
            })),
            locks: Arc::new(Mutex::new(LockTable::default())),
        }
    }

    fn namespace_read(&self) -> io::Result<RwLockReadGuard<'_, Namespace>> {
        self.namespace
            .read()
            .map_err(|_| poisoned_lock("namespace"))
    }

    fn namespace_write(&self) -> io::Result<RwLockWriteGuard<'_, Namespace>> {
        self.namespace
            .write()
            .map_err(|_| poisoned_lock("namespace"))
    }

    fn handles_read(&self) -> io::Result<RwLockReadGuard<'_, HandleTable>> {
        self.handles
            .read()
            .map_err(|_| poisoned_lock("handle table"))
    }

    fn handles_write(&self) -> io::Result<RwLockWriteGuard<'_, HandleTable>> {
        self.handles
            .write()
            .map_err(|_| poisoned_lock("handle table"))
    }

    fn locks_lock(&self) -> io::Result<MutexGuard<'_, LockTable>> {
        self.locks.lock().map_err(|_| poisoned_lock("lock table"))
    }

    fn insert_file_handle(
        &self,
        inode: u64,
        file: File,
        writable: bool,
        lock_path: PathBuf,
    ) -> io::Result<u64> {
        let metadata = fstat_file(&file)?;
        let mut handles = self.handles_write()?;
        let handle = handles.next_handle;
        handles.next_handle = handles.next_handle.saturating_add(1);
        handles.files.insert(
            handle,
            FileHandle {
                inode,
                file,
                writable,
                lock_path,
                dev: metadata.dev(),
                ino: metadata.ino(),
            },
        );
        Ok(handle)
    }

    fn with_file_handle<T>(
        &self,
        inode: u64,
        handle: u64,
        f: impl FnOnce(&FileHandle) -> io::Result<T>,
    ) -> io::Result<T> {
        let handles = self.handles_read()?;
        let file = handles
            .files
            .get(&handle)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        if file.inode != inode {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        f(file)
    }

    fn lock_file_for(&self, inode: u64, handle: u64, owner: u64) -> io::Result<Arc<File>> {
        let key = {
            let handles = self.handles_read()?;
            let handle_file = handles
                .files
                .get(&handle)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
            if handle_file.inode != inode {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            LockKey {
                dev: handle_file.dev,
                ino: handle_file.ino,
                owner,
            }
        };

        let mut locks = self.locks_lock()?;
        if let Some(file) = locks.files.get(&key) {
            return Ok(file.clone());
        }

        let file = {
            let handles = self.handles_read()?;
            let handle_file = handles
                .files
                .get(&handle)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
            if handle_file.inode != inode {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            reopen_lock_file(handle_file)?
        };
        let file = Arc::new(file);
        locks.files.insert(key, file.clone());
        Ok(file)
    }

    fn remove_locks_for_owner(&self, inode: u64, handle: u64, owner: u64) -> io::Result<()> {
        let (dev, ino) = self.with_file_handle(inode, handle, |file| Ok((file.dev, file.ino)))?;
        let mut locks = self.locks_lock()?;
        locks
            .files
            .retain(|key, _| key.dev != dev || key.ino != ino || key.owner != owner);
        Ok(())
    }

    fn remove_released_file_locks(
        &self,
        dev: u64,
        ino: u64,
        owner: Option<u64>,
        has_remaining_handle: bool,
    ) -> io::Result<()> {
        let mut locks = self.locks_lock()?;
        if let Some(owner) = owner {
            locks
                .files
                .retain(|key, _| key.dev != dev || key.ino != ino || key.owner != owner);
        } else if !has_remaining_handle {
            locks
                .files
                .retain(|key, _| key.dev != dev || key.ino != ino);
        }
        Ok(())
    }
}

impl FileSystem for ComposedFs {
    type Inode = u64;
    type Handle = u64;
    type DirIter = VecDirIter;

    fn init(&self, capable: FsOptions) -> io::Result<FsOptions> {
        Ok(capable & (FsOptions::BIG_WRITES | FsOptions::POSIX_LOCKS))
    }

    fn getlk(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        _handle: Self::Handle,
        _owner: u64,
        _lock: fuse::FileLock,
        _flags: u32,
    ) -> io::Result<fuse::FileLock> {
        let file = self.lock_file_for(_inode, _handle, _owner)?;
        let host_lock = fuse_lock_to_host(_lock)?;
        let result = fcntl_ofd_lock(file.as_raw_fd(), libc::F_OFD_GETLK, host_lock)?;
        Ok(host_lock_to_fuse(result)?)
    }

    fn setlk(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        _handle: Self::Handle,
        _owner: u64,
        _lock: fuse::FileLock,
        _flags: u32,
    ) -> io::Result<()> {
        let file = self.lock_file_for(_inode, _handle, _owner)?;
        let host_lock = fuse_lock_to_host(_lock)?;
        fcntl_ofd_lock(file.as_raw_fd(), libc::F_OFD_SETLK, host_lock)?;
        Ok(())
    }

    fn setlkw(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        _handle: Self::Handle,
        _owner: u64,
        _lock: fuse::FileLock,
        _flags: u32,
    ) -> io::Result<()> {
        let file = self.lock_file_for(_inode, _handle, _owner)?;
        let host_lock = fuse_lock_to_host(_lock)?;
        fcntl_ofd_lock_wait_bounded(file.as_raw_fd(), host_lock)?;
        Ok(())
    }

    fn lookup(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<Entry> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        validate_single_path_component(name)?;
        let mut namespace = self.namespace_write()?;
        let parent_node = namespace
            .nodes
            .get(&parent)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?
            .kind
            .clone();
        let parent_snapshot = Node {
            inode: parent,
            kind: parent_node,
            mode: 0,
            uid: 0,
            gid: 0,
            lookup_count: 0,
            cached_attr: None,
        };
        if let Some(inode) = namespace
            .children
            .get(&parent)
            .and_then(|children| children.get(name))
            .copied()
        {
            let entry = {
                let node = namespace
                    .nodes
                    .get_mut(&inode)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                node.lookup_count = node.lookup_count.saturating_add(1);
                let node = namespace
                    .nodes
                    .get(&inode)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                namespace.entry_for(node)?
            };
            return Ok(entry);
        }
        if let Some((_inode, entry)) = namespace.lookup_host_child(&parent_snapshot, name)? {
            return Ok(entry);
        }
        Err(io::Error::from_raw_os_error(libc::ENOENT))
    }

    fn forget(&self, _ctx: Context, inode: Self::Inode, count: u64) {
        let Ok(mut namespace) = self.namespace_write() else {
            return;
        };
        namespace.forget_inode(inode, count);
    }

    fn batch_forget(&self, _ctx: Context, requests: Vec<(Self::Inode, u64)>) {
        let Ok(mut namespace) = self.namespace_write() else {
            return;
        };
        for (inode, count) in requests {
            namespace.forget_inode(inode, count);
        }
    }

    fn getattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _handle: Option<Self::Handle>,
    ) -> io::Result<(fuse::Attr, Duration)> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        Ok((namespace.attr_for(node)?, ATTR_TTL))
    }

    fn setattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        attr: fuse::SetattrIn,
        _handle: Option<Self::Handle>,
        valid: SetattrValid,
    ) -> io::Result<(fuse::Attr, Duration)> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount_index, relative_path)) = namespace
            .shadow_file_location(node)
            .or_else(|| namespace.host_file_location(node))
            .or_else(|| namespace.host_location(node))
        else {
            if valid.is_empty() {
                return Ok((namespace.attr_for(node)?, ATTR_TTL));
            }
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        };
        if setattr_wants_mutation(valid) {
            namespace.ensure_mount_writable(mount_index)?;
        }
        let open_flags = if valid.contains(SetattrValid::SIZE) {
            libc::O_RDWR
        } else if matches!(node.kind, NodeKind::MountRoot { mount } if matches!(namespace.mounts[mount].kind, MountKind::Dir))
            || matches!(node.kind, NodeKind::OverlayDir { .. })
            || namespace.node_is_dir(node)?
        {
            libc::O_RDONLY | libc::O_DIRECTORY
        } else {
            libc::O_RDONLY
        };
        let identity = namespace.host_identity(node);
        let file = if matches!(node.kind, NodeKind::Shadow { .. }) {
            let shadow_relative = namespace.shadow_relative_path(mount_index, &relative_path)?;
            open_beneath_for_io(
                namespace.shadow_root(mount_index)?.as_raw_fd(),
                &shadow_relative,
                open_flags,
                0,
            )?
        } else {
            let mount = namespace.host_mount(mount_index)?;
            open_host_file_for_io(mount, &relative_path, open_flags, 0)?
        };
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        apply_setattr(&file, attr, valid)?;
        let metadata = fstat_file(&file)?;
        Ok((attr_from_metadata(inode, &metadata), ATTR_TTL))
    }

    fn opendir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
    ) -> io::Result<(Option<Self::Handle>, OpenOptions)> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if !namespace.node_is_dir(node)? {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        Ok((Some(inode), OpenOptions::empty()))
    }

    fn readdir(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _handle: Self::Handle,
        size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        let mut namespace = self.namespace_write()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if !namespace.node_is_dir(node)? {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }

        let entry_budget = readdir_entry_budget(size);
        let collection_limit = readdir_collection_limit(offset, entry_budget);
        let mut entries = Vec::new();
        let mut seen = HashSet::<String>::new();
        let mut entry_offset = 0_u64;
        if let Some(children) = namespace.children.get(&inode) {
            for (name, child_inode) in children {
                if !seen.insert(name.clone()) {
                    continue;
                }
                entry_offset = entry_offset.saturating_add(1);
                if entry_offset <= offset {
                    continue;
                }
                if entries.len() >= entry_budget {
                    break;
                }
                let child = namespace
                    .nodes
                    .get(child_inode)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                let type_ = namespace.dirent_type_for(child)?;
                entries.push(OwnedDirEntry {
                    ino: *child_inode,
                    offset: entry_offset,
                    type_,
                    name: CString::new(name.as_str())
                        .map_err(|_| invalid_input(format!("invalid dir entry name {name:?}")))?,
                });
            }
        }
        if entries.len() < entry_budget {
            if let Some((mount_index, relative_path)) = namespace.host_location(node) {
                let (host_root_fd, has_shadow_root) = {
                    let mount = namespace
                        .mounts
                        .get(mount_index)
                        .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                    (mount.root.as_raw_fd(), mount.shadow_root.is_some())
                };
                for entry in read_dir_beneath(host_root_fd, &relative_path, collection_limit)? {
                    let child_relative = relative_path.join(&entry.name);
                    if namespace.filtered_path(mount_index, &child_relative)? {
                        continue;
                    }
                    if !seen.insert(entry.name.clone()) {
                        continue;
                    }
                    entry_offset = entry_offset.saturating_add(1);
                    if entry_offset <= offset {
                        continue;
                    }
                    if entries.len() >= entry_budget {
                        break;
                    }
                    let child_inode = namespace.get_or_create_host_node(
                        mount_index,
                        child_relative,
                        &entry.metadata,
                    );
                    entries.push(OwnedDirEntry {
                        ino: child_inode,
                        offset: entry_offset,
                        type_: dirent_type_from_mode(entry.metadata.mode()),
                        name: CString::new(entry.name.as_str()).map_err(|_| {
                            invalid_input(format!("invalid dir entry name {:?}", entry.name))
                        })?,
                    });
                }
                if has_shadow_root {
                    let shadow_parent =
                        namespace.shadow_relative_path(mount_index, &relative_path)?;
                    match read_dir_beneath(
                        namespace.shadow_root(mount_index)?.as_raw_fd(),
                        &shadow_parent,
                        collection_limit,
                    ) {
                        Ok(shadow_entries) => {
                            for entry in shadow_entries {
                                let child_relative = relative_path.join(&entry.name);
                                if !namespace.filtered_path(mount_index, &child_relative)? {
                                    continue;
                                }
                                if !seen.insert(entry.name.clone()) {
                                    continue;
                                }
                                entry_offset = entry_offset.saturating_add(1);
                                if entry_offset <= offset {
                                    continue;
                                }
                                if entries.len() >= entry_budget {
                                    break;
                                }
                                let child_inode = namespace.get_or_create_shadow_node(
                                    mount_index,
                                    child_relative,
                                    &entry.metadata,
                                );
                                entries.push(OwnedDirEntry {
                                    ino: child_inode,
                                    offset: entry_offset,
                                    type_: dirent_type_from_mode(entry.metadata.mode()),
                                    name: CString::new(entry.name.as_str()).map_err(|_| {
                                        invalid_input(format!(
                                            "invalid dir entry name {:?}",
                                            entry.name
                                        ))
                                    })?,
                                });
                            }
                        }
                        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        namespace.prune_cold_host_shadow_nodes();

        Ok(VecDirIter { entries, index: 0 })
    }

    fn open(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _kill_priv: bool,
        flags: u32,
    ) -> io::Result<(Option<Self::Handle>, OpenOptions)> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if let Some((mount_index, relative_path)) = namespace.shadow_file_location(node) {
            if open_flags_want_write(flags) {
                namespace.ensure_mount_writable(mount_index)?;
            }
            namespace.ensure_shadow_parent(mount_index, &relative_path)?;
            let shadow_relative = namespace.shadow_relative_path(mount_index, &relative_path)?;
            let file = open_beneath_for_io(
                namespace.shadow_root(mount_index)?.as_raw_fd(),
                &shadow_relative,
                flags as i32,
                0,
            )?;
            let lock_path = namespace
                .shadow_root_path(mount_index)?
                .join(&shadow_relative);
            drop(namespace);
            let handle =
                self.insert_file_handle(inode, file, open_flags_want_write(flags), lock_path)?;
            return Ok((Some(handle), OpenOptions::empty()));
        }
        let Some((mount_index, relative_path)) = namespace.host_file_location(node) else {
            return Err(io::Error::from_raw_os_error(libc::EISDIR));
        };
        if matches!(node.kind, NodeKind::MountRoot { mount } if matches!(namespace.mounts[mount].kind, MountKind::Dir))
        {
            return Err(io::Error::from_raw_os_error(libc::EISDIR));
        }
        if open_flags_want_write(flags) {
            namespace.ensure_mount_writable(mount_index)?;
        }
        let identity = namespace.host_identity(node);
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, flags as i32, 0)?;
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        let lock_path = mount.root_path.join(&relative_path);
        drop(namespace);

        let handle =
            self.insert_file_handle(inode, file, open_flags_want_write(flags), lock_path)?;
        Ok((Some(handle), OpenOptions::empty()))
    }

    #[allow(clippy::too_many_arguments)]
    fn create(
        &self,
        _ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        _kill_priv: bool,
        flags: u32,
        umask: u32,
        _extensions: Extensions,
    ) -> io::Result<(Entry, Option<Self::Handle>, OpenOptions)> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut namespace = self.namespace_write()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let create_flags = flags as i32 | libc::O_CREAT;
        let create_mode = mode & !umask;
        if namespace.filtered_path(mount_index, &relative_path)? {
            namespace.ensure_shadow_parent(mount_index, &relative_path)?;
            let shadow_relative = namespace.shadow_relative_path(mount_index, &relative_path)?;
            let file = open_beneath_for_io(
                namespace.shadow_root(mount_index)?.as_raw_fd(),
                &shadow_relative,
                create_flags,
                create_mode,
            )?;
            let metadata = fstat_file(&file)?;
            let lock_path = namespace
                .shadow_root_path(mount_index)?
                .join(&shadow_relative);
            let entry = namespace.entry_for_shadow_metadata(mount_index, relative_path, &metadata);
            let inode = entry.inode;
            drop(namespace);

            let handle =
                self.insert_file_handle(inode, file, open_flags_want_write(flags), lock_path)?;
            return Ok((entry, Some(handle), OpenOptions::empty()));
        }
        let mount = namespace.host_mount(mount_index)?;
        let file = open_beneath_for_io(
            mount.root.as_raw_fd(),
            &relative_path,
            create_flags,
            create_mode,
        )?;
        let metadata = fstat_file(&file)?;
        let lock_path = mount.root_path.join(&relative_path);
        let entry = namespace.entry_for_host_metadata(mount_index, relative_path, &metadata);
        let inode = entry.inode;
        drop(namespace);

        let handle =
            self.insert_file_handle(inode, file, open_flags_want_write(flags), lock_path)?;
        Ok((entry, Some(handle), OpenOptions::empty()))
    }

    fn readlink(&self, _ctx: Context, inode: Self::Inode) -> io::Result<Vec<u8>> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount_index, relative_path)) = namespace.host_file_location(node) else {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        };
        namespace.host_metadata_for(node)?;
        let mount = namespace.host_mount(mount_index)?;
        readlink_beneath(mount.root.as_raw_fd(), &relative_path)
    }

    fn symlink(
        &self,
        _ctx: Context,
        linkname: &CStr,
        parent: Self::Inode,
        name: &CStr,
        _extensions: Extensions,
    ) -> io::Result<Entry> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut namespace = self.namespace_write()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        symlink_beneath(mount.root.as_raw_fd(), linkname, &relative_path)?;
        let metadata = stat_beneath(mount.root.as_raw_fd(), &relative_path)?;
        Ok(namespace.entry_for_host_metadata(mount_index, relative_path, &metadata))
    }

    fn mkdir(
        &self,
        _ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        umask: u32,
        _extensions: Extensions,
    ) -> io::Result<Entry> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut namespace = self.namespace_write()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        mkdir_beneath(mount.root.as_raw_fd(), &relative_path, mode & !umask)?;
        let metadata = stat_beneath(mount.root.as_raw_fd(), &relative_path)?;
        Ok(namespace.entry_for_host_metadata(mount_index, relative_path, &metadata))
    }

    fn mknod(
        &self,
        _ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        rdev: u32,
        umask: u32,
        _extensions: Extensions,
    ) -> io::Result<Entry> {
        let kind = mode & libc::S_IFMT;
        if kind != 0 && kind != libc::S_IFREG && kind != libc::S_IFIFO {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut namespace = self.namespace_write()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        mknod_beneath(mount.root.as_raw_fd(), &relative_path, mode & !umask, rdev)?;
        let metadata = stat_beneath(mount.root.as_raw_fd(), &relative_path)?;
        Ok(namespace.entry_for_host_metadata(mount_index, relative_path, &metadata))
    }

    fn unlink(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        if namespace.filtered_path(mount_index, &relative_path)? {
            let shadow_relative = namespace.shadow_relative_path(mount_index, &relative_path)?;
            return unlink_beneath(
                namespace.shadow_root(mount_index)?.as_raw_fd(),
                &shadow_relative,
                false,
            );
        }
        let mount = namespace.host_mount(mount_index)?;
        unlink_beneath(mount.root.as_raw_fd(), &relative_path, false)
    }

    fn rmdir(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        unlink_beneath(mount.root.as_raw_fd(), &relative_path, true)
    }

    fn rename(
        &self,
        _ctx: Context,
        olddir: Self::Inode,
        oldname: &CStr,
        newdir: Self::Inode,
        newname: &CStr,
        flags: u32,
    ) -> io::Result<()> {
        let oldname = oldname
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let newname = newname
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let namespace = self.namespace_read()?;
        let (old_mount, old_relative) = namespace.host_child_location(olddir, oldname)?;
        let (new_mount, new_relative) = namespace.host_child_location(newdir, newname)?;
        if old_mount != new_mount {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        namespace.ensure_mount_writable(old_mount)?;
        let old_filtered = namespace.filtered_path(old_mount, &old_relative)?;
        let new_filtered = namespace.filtered_path(new_mount, &new_relative)?;
        match (old_filtered, new_filtered) {
            (true, true) => {
                namespace.ensure_shadow_parent(new_mount, &new_relative)?;
                let old_shadow = namespace.shadow_relative_path(old_mount, &old_relative)?;
                let new_shadow = namespace.shadow_relative_path(new_mount, &new_relative)?;
                rename_beneath(
                    namespace.shadow_root(old_mount)?.as_raw_fd(),
                    &old_shadow,
                    &new_shadow,
                    flags,
                )
            }
            (true, false) | (false, true) => Err(io::Error::from_raw_os_error(libc::EXDEV)),
            (false, false) => {
                let mount = namespace.host_mount(old_mount)?;
                rename_beneath(mount.root.as_raw_fd(), &old_relative, &new_relative, flags)
            }
        }
    }

    fn link(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        newparent: Self::Inode,
        newname: &CStr,
    ) -> io::Result<Entry> {
        let newname = newname
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut namespace = self.namespace_write()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((old_mount, old_relative)) = namespace.host_file_location(node) else {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        };
        let (new_mount, new_relative) = namespace.host_child_location(newparent, newname)?;
        if old_mount != new_mount {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        if namespace.filtered_path(new_mount, &new_relative)? {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        namespace.host_metadata_for(node)?;
        namespace.ensure_mount_writable(old_mount)?;
        let mount = namespace.host_mount(old_mount)?;
        link_beneath(mount.root.as_raw_fd(), &old_relative, &new_relative)?;
        let metadata = stat_beneath(mount.root.as_raw_fd(), &new_relative)?;
        Ok(namespace.entry_for_host_metadata(new_mount, new_relative, &metadata))
    }

    fn read<W: ZeroCopyWriter>(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        mut w: W,
        size: u32,
        offset: u64,
        _lock_owner: Option<u64>,
        _flags: u32,
    ) -> io::Result<usize> {
        self.with_file_handle(inode, handle, |file| {
            w.read_from_file_at(&file.file, size as usize, offset, None)
        })
    }

    fn write<R: ZeroCopyReader>(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        mut r: R,
        size: u32,
        offset: u64,
        _lock_owner: Option<u64>,
        _delayed_write: bool,
        _kill_priv: bool,
        _flags: u32,
    ) -> io::Result<usize> {
        self.with_file_handle(inode, handle, |file| {
            if !file.writable {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            r.write_to_file_at(&file.file, size as usize, offset, None)
        })
    }

    fn flush(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        _lock_owner: u64,
    ) -> io::Result<()> {
        self.with_file_handle(inode, handle, |_file| Ok(()))?;
        self.remove_locks_for_owner(inode, handle, _lock_owner)
    }

    fn fsync(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        datasync: bool,
        handle: Self::Handle,
    ) -> io::Result<()> {
        self.with_file_handle(inode, handle, |file| {
            if datasync {
                file.file.sync_data()
            } else {
                file.file.sync_all()
            }
        })
    }

    fn statfs(&self, _ctx: Context, inode: Self::Inode) -> io::Result<libc::statvfs64> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount_index, _relative_path)) = namespace
            .host_file_location(node)
            .or_else(|| namespace.host_location(node))
        else {
            let mut st: libc::statvfs64 = unsafe { mem::zeroed() };
            st.f_namemax = 255;
            st.f_bsize = 4096;
            return Ok(st);
        };
        let mount = namespace.host_mount(mount_index)?;
        fstatvfs_file(&mount.root)
    }

    fn access(&self, _ctx: Context, inode: Self::Inode, mask: u32) -> io::Result<()> {
        let namespace = self.namespace_read()?;
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if mask & libc::W_OK as u32 != 0 {
            if let Some((mount_index, _relative_path)) = namespace
                .host_file_location(node)
                .or_else(|| namespace.host_location(node))
            {
                namespace.ensure_mount_writable(mount_index)?;
            }
        }
        let Some((mount_index, relative_path)) = namespace
            .host_file_location(node)
            .or_else(|| namespace.host_location(node))
        else {
            return Ok(());
        };
        namespace.host_metadata_for(node)?;
        let mount = namespace.host_mount(mount_index)?;
        access_host_path(mount, &relative_path, mask as i32)
    }

    fn lseek(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        offset: u64,
        whence: u32,
    ) -> io::Result<u64> {
        self.with_file_handle(inode, handle, |file| {
            let result =
                unsafe { libc::lseek64(file.file.as_raw_fd(), offset as i64, whence as i32) };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(result as u64)
        })
    }

    fn setxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        value: &[u8],
        flags: u32,
        _extra_flags: SetxattrFlags,
    ) -> io::Result<()> {
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path, identity) = namespace.xattr_location(inode)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        setxattr_file(&file, name, value, flags)
    }

    fn getxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        size: u32,
    ) -> io::Result<GetxattrReply> {
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path, identity) = namespace.xattr_location(inode)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        getxattr_file(&file, name, size)
    }

    fn listxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        size: u32,
    ) -> io::Result<ListxattrReply> {
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path, identity) = namespace.xattr_location(inode)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        listxattr_file(&file, size)
    }

    fn removexattr(&self, _ctx: Context, inode: Self::Inode, name: &CStr) -> io::Result<()> {
        let namespace = self.namespace_read()?;
        let (mount_index, relative_path, identity) = namespace.xattr_location(inode)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        if let Some(identity) = identity {
            validate_host_identity(&fstat_file(&file)?, identity)?;
        }
        removexattr_file(&file, name)
    }

    fn fsyncdir(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        _datasync: bool,
        _handle: Self::Handle,
    ) -> io::Result<()> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn release(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _flags: u32,
        handle: Self::Handle,
        flush: bool,
        _flock_release: bool,
        _lock_owner: Option<u64>,
    ) -> io::Result<()> {
        let (file, has_remaining_handle) = {
            let mut handles = self.handles_write()?;
            let file = handles
                .files
                .get(&handle)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
            if file.inode != inode {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            let dev = file.dev;
            let ino = file.ino;
            let file = handles
                .files
                .remove(&handle)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
            let has_remaining_handle = handles
                .files
                .values()
                .any(|other| other.dev == dev && other.ino == ino);
            (file, has_remaining_handle)
        };
        if flush && file.writable {
            file.file.sync_all()?;
        }
        let dev = file.dev;
        let ino = file.ino;
        drop(file);
        self.remove_released_file_locks(dev, ino, _lock_owner, has_remaining_handle)
    }

    fn releasedir(
        &self,
        _ctx: Context,
        _inode: Self::Inode,
        _flags: u32,
        _handle: Self::Handle,
    ) -> io::Result<()> {
        Ok(())
    }
}

impl SerializableFileSystem for ComposedFs {}

struct OwnedDirEntry {
    ino: u64,
    offset: u64,
    type_: u32,
    name: CString,
}

struct VecDirIter {
    entries: Vec<OwnedDirEntry>,
    index: usize,
}

impl DirectoryIterator for VecDirIter {
    fn next(&mut self) -> Option<DirEntry<'_>> {
        let entry = self.entries.get(self.index)?;
        self.index += 1;
        Some(DirEntry {
            ino: entry.ino,
            offset: entry.offset,
            type_: entry.type_,
            name: entry.name.as_c_str(),
        })
    }
}

fn parse_octal_mode(value: &str) -> io::Result<u32> {
    let trimmed = value.strip_prefix("0o").unwrap_or(value);
    u32::from_str_radix(trimmed, 8)
        .map_err(|error| invalid_input(format!("invalid octal mode {value:?}: {error}")))
}

fn normalize_guest_path(path: &str) -> io::Result<String> {
    if !path.starts_with('/') {
        return Err(invalid_input(format!(
            "guest_path must be absolute: {path:?}"
        )));
    }
    let components = path_components(path)?;
    if components.is_empty() {
        return Ok(String::from("/"));
    }
    Ok(format!("/{}", components.join("/")))
}

fn path_components(path: &str) -> io::Result<Vec<String>> {
    let mut components = Vec::new();
    for component in path.split('/') {
        if component.is_empty() {
            continue;
        }
        if component == "." || component == ".." {
            return Err(invalid_input(format!(
                "guest_path contains invalid component {component:?}: {path:?}"
            )));
        }
        if component.as_bytes().contains(&0) {
            return Err(invalid_input(format!(
                "guest_path contains NUL byte in component: {path:?}"
            )));
        }
        components.push(component.to_string());
    }
    Ok(components)
}

fn validate_single_path_component(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }
    if name.as_bytes().contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> io::Result<()> {
    for component in path.components() {
        match component {
            Component::Normal(part) if !part.as_bytes().contains(&0) => {}
            _ => {
                return Err(invalid_input(format!(
                    "unsafe host-relative path component in {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn attr_from_metadata(inode: u64, metadata: &fs::Metadata) -> fuse::Attr {
    fuse::Attr {
        ino: inode,
        size: metadata.size(),
        blocks: metadata.blocks(),
        atime: metadata.atime().max(0) as u64,
        mtime: metadata.mtime().max(0) as u64,
        ctime: metadata.ctime().max(0) as u64,
        atimensec: metadata.atime_nsec() as u32,
        mtimensec: metadata.mtime_nsec() as u32,
        ctimensec: metadata.ctime_nsec() as u32,
        mode: metadata.mode(),
        nlink: metadata.nlink() as u32,
        uid: GuestUid::from(metadata.uid()),
        gid: GuestGid::from(metadata.gid()),
        rdev: metadata.rdev() as u32,
        blksize: metadata.blksize() as u32,
        flags: 0,
    }
}

fn dirent_type_from_mode(mode: u32) -> u32 {
    match mode & libc::S_IFMT {
        x if x == libc::S_IFDIR => libc::DT_DIR as u32,
        x if x == libc::S_IFREG => libc::DT_REG as u32,
        x if x == libc::S_IFLNK => libc::DT_LNK as u32,
        x if x == libc::S_IFIFO => libc::DT_FIFO as u32,
        x if x == libc::S_IFSOCK => libc::DT_SOCK as u32,
        x if x == libc::S_IFCHR => libc::DT_CHR as u32,
        x if x == libc::S_IFBLK => libc::DT_BLK as u32,
        _ => libc::DT_UNKNOWN as u32,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn poisoned_lock(name: &str) -> io::Error {
    io::Error::other(format!("{name} lock poisoned"))
}

#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzz_harness;

#[cfg(any(test, feature = "fuzzing"))]
mod test_support;

#[cfg(test)]
mod tests;
