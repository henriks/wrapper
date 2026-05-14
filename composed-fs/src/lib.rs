use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use virtiofsd::filesystem::{
    Context, DirEntry, DirectoryIterator, Entry, Extensions, FileSystem, FsOptions, GetxattrReply,
    ListxattrReply, OpenOptions, SerializableFileSystem, SetattrValid, SetxattrFlags,
    ZeroCopyReader, ZeroCopyWriter, ROOT_ID,
};
use virtiofsd::fuse;
use virtiofsd::soft_idmap::{GuestGid, GuestUid, Id};
use virtiofsd::vhost_user::VhostUserFsBackendBuilder;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};

const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TAG: &str = "agentvm";
pub const DEFAULT_THREAD_POOL_SIZE: usize = 1;
const ATTR_TTL: Duration = Duration::from_secs(1);
const ENTRY_TTL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    #[serde(default)]
    export_tag: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    mounts: Vec<MountSpec>,
    #[serde(default)]
    synthetic: Option<SyntheticSpec>,
    #[serde(default)]
    protected_guest_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MountSpec {
    id: String,
    guest_path: String,
    host_path: String,
    kind: MountKind,
    access: AccessMode,
    source_class: SourceClass,
    required: bool,
    bind: bool,
    metadata: MetadataSpec,
}

impl MountSpec {
    fn validate(&self) -> io::Result<()> {
        if self.id.is_empty() {
            return Err(invalid_input("mount id cannot be empty"));
        }
        if !self.host_path.starts_with('/') {
            return Err(invalid_input(format!(
                "host_path must be absolute for mount {}: {:?}",
                self.id, self.host_path
            )));
        }
        let _access = match self.access {
            AccessMode::Ro => "ro",
            AccessMode::Rw => "rw",
        };
        let _source_class = match self.source_class {
            SourceClass::Workspace => "workspace",
            SourceClass::ToolState => "tool-state",
            SourceClass::AuthConfig => "auth-config",
            SourceClass::SystemRo => "system-ro",
            SourceClass::UserRo => "user-ro",
            SourceClass::UserRw => "user-rw",
        };
        let _required = self.required;
        let _bind = self.bind;
        self.metadata.validate();
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MountKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AccessMode {
    Ro,
    Rw,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SourceClass {
    Workspace,
    ToolState,
    AuthConfig,
    SystemRo,
    UserRo,
    UserRw,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataSpec {
    uid_gid: MetadataPolicy,
    permissions: MetadataPolicy,
}

impl MetadataSpec {
    fn validate(&self) {
        match self.uid_gid {
            MetadataPolicy::Host => {}
        }
        match self.permissions {
            MetadataPolicy::Host => {}
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MetadataPolicy {
    Host,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticSpec {
    uid: u32,
    gid: u32,
    dir_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeKind {
    SyntheticDir,
    OverlayDir {
        mount: usize,
        relative_path: PathBuf,
    },
    MountRoot {
        mount: usize,
    },
    Host {
        mount: usize,
        relative_path: PathBuf,
    },
}

#[derive(Debug)]
struct Node {
    inode: u64,
    kind: NodeKind,
    mode: u32,
    uid: u32,
    gid: u32,
    lookup_count: u64,
    cached_attr: Option<fuse::Attr>,
}

#[derive(Debug)]
#[allow(dead_code)]
struct MountRuntime {
    id: String,
    root_path: PathBuf,
    root: File,
    kind: MountKind,
    access: AccessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HostKey {
    mount: usize,
    dev: u64,
    ino: u64,
}

#[derive(Debug)]
struct Namespace {
    nodes: HashMap<u64, Node>,
    children: HashMap<u64, BTreeMap<String, u64>>,
    path_to_inode: HashMap<String, u64>,
    mounts: Vec<MountRuntime>,
    host_inodes: HashMap<HostKey, u64>,
    next_inode: u64,
    synthetic_uid: u32,
    synthetic_gid: u32,
    synthetic_dir_mode: u32,
}

struct FileHandle {
    inode: u64,
    file: File,
    writable: bool,
}

#[derive(Default)]
struct HandleTable {
    next_handle: u64,
    files: HashMap<u64, FileHandle>,
}

impl Namespace {
    fn from_manifest(manifest: &Manifest) -> io::Result<Self> {
        if manifest.schema_version != SCHEMA_VERSION {
            return Err(invalid_input(format!(
                "unsupported schema_version {}, expected {}",
                manifest.schema_version, SCHEMA_VERSION
            )));
        }
        let _created_by = manifest.created_by.as_deref().unwrap_or("unknown");
        let _export_tag = manifest.export_tag.as_deref().unwrap_or(DEFAULT_TAG);
        if manifest.mounts.is_empty() {
            return Err(invalid_input("manifest must contain at least one mount"));
        }

        let synthetic = manifest.synthetic.as_ref();
        let synthetic_uid = synthetic.map_or(0, |s| s.uid);
        let synthetic_gid = synthetic.map_or(0, |s| s.gid);
        let synthetic_dir_mode = synthetic
            .map(|s| parse_octal_mode(&s.dir_mode))
            .transpose()?
            .unwrap_or(0o555);

        let mut ns = Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            path_to_inode: HashMap::new(),
            mounts: Vec::new(),
            host_inodes: HashMap::new(),
            next_inode: ROOT_ID + 1,
            synthetic_uid,
            synthetic_gid,
            synthetic_dir_mode,
        };
        ns.insert_root();

        for mount in &manifest.mounts {
            mount.validate()?;
            let guest_path = normalize_guest_path(&mount.guest_path)?;
            if manifest
                .protected_guest_paths
                .iter()
                .any(|protected| protected == &guest_path)
            {
                return Err(invalid_input(format!(
                    "mount targets protected guest path: {guest_path}"
                )));
            }
            if guest_path == "/" {
                return Err(invalid_input("mount guest_path cannot be /"));
            }
            if ns.path_to_inode.contains_key(&guest_path) {
                return Err(invalid_input(format!(
                    "duplicate guest_path in manifest: {guest_path}"
                )));
            }
            let mount_index = ns.mounts.len();
            let root = open_mount_root(Path::new(&mount.host_path), mount.kind)?;
            ns.mounts.push(MountRuntime {
                id: mount.id.clone(),
                root_path: PathBuf::from(&mount.host_path),
                root,
                kind: mount.kind,
                access: mount.access,
            });

            let components = path_components(&guest_path)?;
            let mut parent = ROOT_ID;
            let mut current_path = String::new();
            for component in &components[..components.len() - 1] {
                current_path.push('/');
                current_path.push_str(component);
                parent = ns.ensure_synthetic_dir(parent, component, &current_path)?;
            }

            let leaf = components.last().expect("non-root path has leaf");
            let inode = ns.allocate_inode();
            let mode = match mount.kind {
                MountKind::Dir => libc::S_IFDIR as u32 | 0o755,
                MountKind::File => libc::S_IFREG as u32 | 0o444,
            };
            ns.insert_node(Node {
                inode,
                kind: NodeKind::MountRoot { mount: mount_index },
                mode,
                uid: synthetic_uid,
                gid: synthetic_gid,
                lookup_count: 0,
                cached_attr: None,
            });
            ns.link_child(parent, leaf, inode)?;
            ns.path_to_inode.insert(guest_path, inode);
        }

        Ok(ns)
    }

    fn insert_root(&mut self) {
        self.nodes.insert(
            ROOT_ID,
            Node {
                inode: ROOT_ID,
                kind: NodeKind::SyntheticDir,
                mode: libc::S_IFDIR as u32 | self.synthetic_dir_mode,
                uid: self.synthetic_uid,
                gid: self.synthetic_gid,
                lookup_count: 1,
                cached_attr: None,
            },
        );
        self.children.entry(ROOT_ID).or_default();
        self.path_to_inode.insert(String::from("/"), ROOT_ID);
    }

    fn ensure_synthetic_dir(
        &mut self,
        parent: u64,
        name: &str,
        guest_path: &str,
    ) -> io::Result<u64> {
        if let Some(inode) = self
            .children
            .get(&parent)
            .and_then(|c| c.get(name))
            .copied()
        {
            let node = self.nodes.get(&inode).expect("child inode must exist");
            if !self.node_is_dir(node)? {
                return Err(invalid_input(format!(
                    "path component {guest_path} conflicts with non-directory manifest entry"
                )));
            }
            return Ok(inode);
        }

        let inode = self.allocate_inode();
        let kind = match self.nodes.get(&parent).map(|node| &node.kind) {
            Some(NodeKind::MountRoot { mount }) => NodeKind::OverlayDir {
                mount: *mount,
                relative_path: PathBuf::from(name),
            },
            Some(NodeKind::OverlayDir {
                mount,
                relative_path,
            }) => NodeKind::OverlayDir {
                mount: *mount,
                relative_path: relative_path.join(name),
            },
            Some(NodeKind::Host {
                mount,
                relative_path,
            }) => NodeKind::OverlayDir {
                mount: *mount,
                relative_path: relative_path.join(name),
            },
            _ => NodeKind::SyntheticDir,
        };
        self.insert_node(Node {
            inode,
            kind,
            mode: libc::S_IFDIR as u32 | self.synthetic_dir_mode,
            uid: self.synthetic_uid,
            gid: self.synthetic_gid,
            lookup_count: 0,
            cached_attr: None,
        });
        self.link_child(parent, name, inode)?;
        self.path_to_inode.insert(guest_path.to_string(), inode);
        Ok(inode)
    }

    fn insert_node(&mut self, node: Node) {
        if matches!(
            node.kind,
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } | NodeKind::MountRoot { .. }
        ) {
            self.children.entry(node.inode).or_default();
        }
        self.nodes.insert(node.inode, node);
    }

    fn link_child(&mut self, parent: u64, name: &str, child: u64) -> io::Result<()> {
        let children = self
            .children
            .get_mut(&parent)
            .ok_or_else(|| invalid_input(format!("parent inode {parent} is not a directory")))?;
        if children.insert(name.to_string(), child).is_some() {
            return Err(invalid_input(format!(
                "duplicate child {name:?} under parent inode {parent}"
            )));
        }
        Ok(())
    }

    fn allocate_inode(&mut self) -> u64 {
        let inode = self.next_inode;
        self.next_inode += 1;
        inode
    }

    fn attr_for(&self, node: &Node) -> io::Result<fuse::Attr> {
        match self.host_metadata_for(node) {
            Ok(Some(metadata)) => return Ok(attr_from_metadata(node.inode, &metadata)),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                if let Some(attr) = node.cached_attr {
                    return Ok(attr);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }

        let now = now_secs();
        let nlink = if matches!(
            node.kind,
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. }
        ) {
            2
        } else {
            1
        };
        Ok(fuse::Attr {
            ino: node.inode,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: node.mode,
            nlink,
            uid: GuestUid::from(node.uid),
            gid: GuestGid::from(node.gid),
            rdev: 0,
            blksize: 4096,
            flags: 0,
        })
    }

    fn entry_for(&self, node: &Node) -> io::Result<Entry> {
        Ok(Entry {
            inode: node.inode,
            generation: 0,
            attr: self.attr_for(node)?,
            attr_timeout: ATTR_TTL,
            entry_timeout: ENTRY_TTL,
        })
    }

    fn host_metadata_for(&self, node: &Node) -> io::Result<Option<fs::Metadata>> {
        match &node.kind {
            NodeKind::MountRoot { mount } => {
                let mount = self
                    .mounts
                    .get(*mount)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                Ok(Some(fstat_file(&mount.root)?))
            }
            NodeKind::Host {
                mount,
                relative_path,
            }
            | NodeKind::OverlayDir {
                mount,
                relative_path,
            } => {
                let mount = self
                    .mounts
                    .get(*mount)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                Ok(Some(stat_beneath(mount.root.as_raw_fd(), relative_path)?))
            }
            NodeKind::SyntheticDir => Ok(None),
        }
    }

    fn node_is_dir(&self, node: &Node) -> io::Result<bool> {
        Ok(match &node.kind {
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } => true,
            NodeKind::MountRoot { mount } => matches!(self.mounts[*mount].kind, MountKind::Dir),
            NodeKind::Host { .. } => self
                .host_metadata_for(node)?
                .is_some_and(|metadata| metadata.file_type().is_dir()),
        })
    }

    fn host_location(&self, node: &Node) -> Option<(usize, PathBuf)> {
        match &node.kind {
            NodeKind::MountRoot { mount } if matches!(self.mounts[*mount].kind, MountKind::Dir) => {
                Some((*mount, PathBuf::new()))
            }
            NodeKind::OverlayDir {
                mount,
                relative_path,
            }
            | NodeKind::Host {
                mount,
                relative_path,
            } => Some((*mount, relative_path.clone())),
            _ => None,
        }
    }

    fn host_file_location(&self, node: &Node) -> Option<(usize, PathBuf)> {
        match &node.kind {
            NodeKind::MountRoot { mount } => Some((*mount, PathBuf::new())),
            NodeKind::Host {
                mount,
                relative_path,
            } => Some((*mount, relative_path.clone())),
            _ => None,
        }
    }

    fn host_child_location(&self, parent: u64, name: &str) -> io::Result<(usize, PathBuf)> {
        validate_single_path_component(name)?;
        let parent = self
            .nodes
            .get(&parent)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount, relative_path)) = self.host_location(parent) else {
            return Err(io::Error::from_raw_os_error(libc::EROFS));
        };
        Ok((mount, relative_path.join(name)))
    }

    fn host_mount(&self, mount: usize) -> io::Result<&MountRuntime> {
        self.mounts
            .get(mount)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    fn xattr_location(&self, inode: u64) -> io::Result<(usize, PathBuf)> {
        let node = self
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        self.host_file_location(node)
            .or_else(|| self.host_location(node))
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }

    fn ensure_mount_writable(&self, mount: usize) -> io::Result<()> {
        match self.host_mount(mount)?.access {
            AccessMode::Rw => Ok(()),
            AccessMode::Ro => Err(io::Error::from_raw_os_error(libc::EROFS)),
        }
    }

    fn lookup_host_child(&mut self, parent: &Node, name: &str) -> io::Result<Option<(u64, Entry)>> {
        let Some((mount_index, parent_relative)) = self.host_location(parent) else {
            return Ok(None);
        };
        let mount = self
            .mounts
            .get(mount_index)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if !self.node_is_dir(parent)? {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        let relative_path = parent_relative.join(name);
        let metadata = match stat_beneath(mount.root.as_raw_fd(), &relative_path) {
            Ok(metadata) => metadata,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
            Err(error) => return Err(error),
        };
        let inode = self.get_or_create_host_node(mount_index, relative_path.clone(), &metadata);
        let node = self
            .nodes
            .get_mut(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        node.lookup_count = node.lookup_count.saturating_add(1);
        let entry = Entry {
            inode,
            generation: 0,
            attr: attr_from_metadata(inode, &metadata),
            attr_timeout: ATTR_TTL,
            entry_timeout: ENTRY_TTL,
        };
        Ok(Some((inode, entry)))
    }

    fn get_or_create_host_node(
        &mut self,
        mount_index: usize,
        relative_path: PathBuf,
        metadata: &fs::Metadata,
    ) -> u64 {
        let key = HostKey {
            mount: mount_index,
            dev: metadata.dev(),
            ino: metadata.ino(),
        };
        if let Some(inode) = self.host_inodes.get(&key).copied() {
            if let Some(node) = self.nodes.get_mut(&inode) {
                if let NodeKind::Host {
                    mount,
                    relative_path: known_path,
                } = &mut node.kind
                {
                    if *mount == mount_index {
                        *known_path = relative_path;
                    }
                }
                node.mode = metadata.mode();
                node.uid = metadata.uid();
                node.gid = metadata.gid();
                node.cached_attr = Some(attr_from_metadata(inode, metadata));
            }
            return inode;
        }

        let inode = self.allocate_inode();
        self.insert_node(Node {
            inode,
            kind: NodeKind::Host {
                mount: mount_index,
                relative_path,
            },
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            lookup_count: 0,
            cached_attr: Some(attr_from_metadata(inode, metadata)),
        });
        self.host_inodes.insert(key, inode);
        inode
    }

    fn entry_for_host_metadata(
        &mut self,
        mount_index: usize,
        relative_path: PathBuf,
        metadata: &fs::Metadata,
    ) -> Entry {
        let inode = self.get_or_create_host_node(mount_index, relative_path, metadata);
        let node = self
            .nodes
            .get_mut(&inode)
            .expect("host node must exist after insertion");
        node.lookup_count = node.lookup_count.saturating_add(1);
        node.cached_attr = Some(attr_from_metadata(inode, metadata));
        Entry {
            inode,
            generation: 0,
            attr: attr_from_metadata(inode, metadata),
            attr_timeout: ATTR_TTL,
            entry_timeout: ENTRY_TTL,
        }
    }

    fn forget_inode(&mut self, inode: u64, count: u64) {
        if inode == ROOT_ID {
            return;
        }
        if let Some(node) = self.nodes.get_mut(&inode) {
            node.lookup_count = node.lookup_count.saturating_sub(count);
        }
    }

    fn dirent_type_for(&self, node: &Node) -> io::Result<u32> {
        Ok(match &node.kind {
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } => libc::DT_DIR as u32,
            NodeKind::MountRoot { mount } => match self.mounts[*mount].kind {
                MountKind::Dir => libc::DT_DIR as u32,
                MountKind::File => libc::DT_REG as u32,
            },
            NodeKind::Host { .. } => {
                let metadata = self
                    .host_metadata_for(node)?
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                dirent_type_from_mode(metadata.mode())
            }
        })
    }
}

#[derive(Clone)]
struct ComposedFs {
    namespace: Arc<RwLock<Namespace>>,
    handles: Arc<RwLock<HandleTable>>,
}

impl ComposedFs {
    fn new(namespace: Namespace) -> Self {
        Self {
            namespace: Arc::new(RwLock::new(namespace)),
            handles: Arc::new(RwLock::new(HandleTable {
                next_handle: 1,
                files: HashMap::new(),
            })),
        }
    }

    fn insert_file_handle(&self, inode: u64, file: File, writable: bool) -> u64 {
        let mut handles = self.handles.write().expect("handle table lock poisoned");
        let handle = handles.next_handle;
        handles.next_handle = handles.next_handle.saturating_add(1);
        handles.files.insert(
            handle,
            FileHandle {
                inode,
                file,
                writable,
            },
        );
        handle
    }

    fn with_file_handle<T>(
        &self,
        inode: u64,
        handle: u64,
        f: impl FnOnce(&FileHandle) -> io::Result<T>,
    ) -> io::Result<T> {
        let handles = self.handles.read().expect("handle table lock poisoned");
        let file = handles
            .files
            .get(&handle)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        if file.inode != inode {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        f(file)
    }
}

impl FileSystem for ComposedFs {
    type Inode = u64;
    type Handle = u64;
    type DirIter = VecDirIter;

    fn init(&self, capable: FsOptions) -> io::Result<FsOptions> {
        Ok(capable & FsOptions::BIG_WRITES)
    }

    fn lookup(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<Entry> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        validate_single_path_component(name)?;
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
        namespace.forget_inode(inode, count);
    }

    fn batch_forget(&self, _ctx: Context, requests: Vec<(Self::Inode, u64)>) {
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount_index, relative_path)) = namespace
            .host_file_location(node)
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
        let mount = namespace.host_mount(mount_index)?;
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
        let file = open_host_file_for_io(mount, &relative_path, open_flags, 0)?;
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
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
        _size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if !namespace.node_is_dir(node)? {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }

        let mut entries = Vec::new();
        let mut listed = BTreeMap::<String, (u64, u32)>::new();
        if let Some(children) = namespace.children.get(&inode) {
            for (name, child_inode) in children {
                let child = namespace
                    .nodes
                    .get(child_inode)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                let type_ = namespace.dirent_type_for(child)?;
                listed.insert(name.clone(), (*child_inode, type_));
            }
        }
        if let Some((mount_index, relative_path)) = namespace.host_location(node) {
            let mount = namespace
                .mounts
                .get(mount_index)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
            let host_entries = read_dir_beneath(mount.root.as_raw_fd(), &relative_path)?;
            for entry in host_entries {
                let child_relative = relative_path.join(&entry.name);
                let child_inode =
                    namespace.get_or_create_host_node(mount_index, child_relative, &entry.metadata);
                listed
                    .entry(entry.name)
                    .or_insert((child_inode, dirent_type_from_mode(entry.metadata.mode())));
            }
        }

        for (index, (name, (child_inode, type_))) in listed.iter().enumerate() {
            let entry_offset = (index + 1) as u64;
            if entry_offset <= offset {
                continue;
            }
            entries.push(OwnedDirEntry {
                ino: *child_inode,
                offset: entry_offset,
                type_: *type_,
                name: CString::new(name.as_str())
                    .map_err(|_| invalid_input(format!("invalid dir entry name {name:?}")))?,
            });
        }

        Ok(VecDirIter { entries, index: 0 })
    }

    fn open(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        _kill_priv: bool,
        flags: u32,
    ) -> io::Result<(Option<Self::Handle>, OpenOptions)> {
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
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
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, flags as i32, 0)?;
        drop(namespace);

        let handle = self.insert_file_handle(inode, file, open_flags_want_write(flags));
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        let create_flags = flags as i32 | libc::O_CREAT;
        let create_mode = mode & !umask;
        let file = open_beneath_for_io(
            mount.root.as_raw_fd(),
            &relative_path,
            create_flags,
            create_mode,
        )?;
        let metadata = fstat_file(&file)?;
        let entry = namespace.entry_for_host_metadata(mount_index, relative_path, &metadata);
        let inode = entry.inode;
        drop(namespace);

        let handle = self.insert_file_handle(inode, file, open_flags_want_write(flags));
        Ok((entry, Some(handle), OpenOptions::empty()))
    }

    fn readlink(&self, _ctx: Context, inode: Self::Inode) -> io::Result<Vec<u8>> {
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let node = namespace
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount_index, relative_path)) = namespace.host_file_location(node) else {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        };
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.host_child_location(parent, name)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        unlink_beneath(mount.root.as_raw_fd(), &relative_path, false)
    }

    fn rmdir(&self, _ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let name = name
            .to_str()
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOENT))?;
        let namespace = self.namespace.read().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (old_mount, old_relative) = namespace.host_child_location(olddir, oldname)?;
        let (new_mount, new_relative) = namespace.host_child_location(newdir, newname)?;
        if old_mount != new_mount {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        namespace.ensure_mount_writable(old_mount)?;
        let mount = namespace.host_mount(old_mount)?;
        rename_beneath(mount.root.as_raw_fd(), &old_relative, &new_relative, flags)
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
        let mut namespace = self.namespace.write().expect("namespace lock poisoned");
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
        self.with_file_handle(inode, handle, |_file| Ok(()))
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
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
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.xattr_location(inode)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        setxattr_file(&file, name, value, flags)
    }

    fn getxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        size: u32,
    ) -> io::Result<GetxattrReply> {
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.xattr_location(inode)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        getxattr_file(&file, name, size)
    }

    fn listxattr(
        &self,
        _ctx: Context,
        inode: Self::Inode,
        size: u32,
    ) -> io::Result<ListxattrReply> {
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.xattr_location(inode)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
        listxattr_file(&file, size)
    }

    fn removexattr(&self, _ctx: Context, inode: Self::Inode, name: &CStr) -> io::Result<()> {
        let namespace = self.namespace.read().expect("namespace lock poisoned");
        let (mount_index, relative_path) = namespace.xattr_location(inode)?;
        namespace.ensure_mount_writable(mount_index)?;
        let mount = namespace.host_mount(mount_index)?;
        let file = open_host_file_for_io(mount, &relative_path, libc::O_RDONLY, 0)?;
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
        let mut handles = self.handles.write().expect("handle table lock poisoned");
        let file_inode = handles
            .files
            .get(&handle)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?
            .inode;
        if file_inode != inode {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        let file = handles
            .files
            .remove(&handle)
            .expect("handle was checked before removal");
        if flush && file.writable {
            file.file.sync_all()?;
        }
        Ok(())
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

#[derive(Debug)]
struct Args {
    manifest: PathBuf,
    socket_path: PathBuf,
    tag: String,
    thread_pool_size: usize,
}

impl Args {
    fn parse() -> io::Result<Self> {
        let mut manifest = None;
        let mut socket_path = None;
        let mut tag = String::from(DEFAULT_TAG);
        let mut thread_pool_size = DEFAULT_THREAD_POOL_SIZE;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--manifest" => manifest = Some(next_arg(&mut args, "--manifest")?.into()),
                "--socket-path" => socket_path = Some(next_arg(&mut args, "--socket-path")?.into()),
                "--tag" => tag = next_arg(&mut args, "--tag")?,
                "--thread-pool-size" => {
                    let value = next_arg(&mut args, "--thread-pool-size")?;
                    thread_pool_size = value.parse::<usize>().map_err(|error| {
                        invalid_input(format!("invalid --thread-pool-size {value:?}: {error}"))
                    })?;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                _ => return Err(invalid_input(format!("unknown argument {arg:?}"))),
            }
        }

        Ok(Self {
            manifest: manifest.ok_or_else(|| invalid_input("--manifest is required"))?,
            socket_path: socket_path.ok_or_else(|| invalid_input("--socket-path is required"))?,
            tag,
            thread_pool_size,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub manifest: PathBuf,
    pub socket_path: PathBuf,
    pub tag: String,
    pub thread_pool_size: usize,
}

impl ServeConfig {
    pub fn new(manifest: PathBuf, socket_path: PathBuf) -> Self {
        Self {
            manifest,
            socket_path,
            tag: String::from(DEFAULT_TAG),
            thread_pool_size: DEFAULT_THREAD_POOL_SIZE,
        }
    }
}

pub fn run_cli() -> io::Result<()> {
    let args = Args::parse()?;
    serve_vhost_user_fs(ServeConfig {
        manifest: args.manifest,
        socket_path: args.socket_path,
        tag: args.tag,
        thread_pool_size: args.thread_pool_size,
    })
}

pub fn serve_vhost_user_fs(config: ServeConfig) -> io::Result<()> {
    let manifest_text = fs::read_to_string(&config.manifest)?;
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .map_err(|error| invalid_input(format!("invalid manifest JSON: {error}")))?;
    let namespace = Namespace::from_manifest(&manifest)?;
    let fs = ComposedFs::new(namespace);

    if let Some(parent) = config.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let backend = Arc::new(
        VhostUserFsBackendBuilder::default()
            .set_thread_pool_size(config.thread_pool_size)
            .set_tag(Some(config.tag.clone()))
            .build(fs)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?,
    );
    let listener = Listener::new(&config.socket_path, true)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let mut daemon = VhostUserDaemon::new(
        String::from("agentvm-composed-fs"),
        backend,
        GuestMemoryAtomic::new(GuestMemoryMmap::new()),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;

    eprintln!(
        "agentvm-composed-fs: serving tag {:?} on {}",
        config.tag,
        config.socket_path.display()
    );
    daemon
        .start(listener)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, format!("{error:?}")))?;
    daemon
        .wait()
        .map_err(|error| io::Error::new(io::ErrorKind::Other, format!("{error:?}")))?;
    Ok(())
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> io::Result<String> {
    args.next()
        .ok_or_else(|| invalid_input(format!("{flag} requires a value")))
}

fn print_usage() {
    println!(
        "usage: agentvm-composed-fs --manifest PATH --socket-path PATH [--tag TAG] [--thread-pool-size N]"
    );
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

fn open_mount_root(path: &Path, kind: MountKind) -> io::Result<File> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(invalid_input(format!(
            "host_path contains NUL byte: {}",
            path.display()
        )));
    }
    let c_path = CString::new(bytes)?;
    let mut flags = libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    if matches!(kind, MountKind::Dir) {
        flags |= libc::O_DIRECTORY;
    }
    let fd = unsafe { libc::open(c_path.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = fstat_file(&file)?;
    match (
        kind,
        metadata.file_type().is_dir(),
        metadata.file_type().is_file(),
    ) {
        (MountKind::Dir, true, _) | (MountKind::File, _, true) => Ok(file),
        (MountKind::Dir, _, _) => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        (MountKind::File, _, _) => Err(io::Error::from_raw_os_error(libc::EINVAL)),
    }
}

fn stat_beneath(root_fd: RawFd, relative_path: &Path) -> io::Result<fs::Metadata> {
    let file = open_beneath(root_fd, relative_path, libc::O_PATH | libc::O_NOFOLLOW)?;
    fstat_file(&file)
}

fn open_beneath(root_fd: RawFd, relative_path: &Path, flags: i32) -> io::Result<File> {
    open_beneath_with_mode(root_fd, relative_path, flags, 0)
}

fn open_beneath_for_io(
    root_fd: RawFd,
    relative_path: &Path,
    flags: i32,
    mode: u32,
) -> io::Result<File> {
    open_beneath_with_mode(root_fd, relative_path, flags, mode)
}

fn open_host_file_for_io(
    mount: &MountRuntime,
    relative_path: &Path,
    flags: i32,
    mode: u32,
) -> io::Result<File> {
    if relative_path.as_os_str().is_empty() && matches!(mount.kind, MountKind::File) {
        return open_absolute_nofollow(&mount.root_path, flags, mode);
    }
    open_beneath_for_io(mount.root.as_raw_fd(), relative_path, flags, mode)
}

fn open_absolute_nofollow(path: &Path, flags: i32, mode: u32) -> io::Result<File> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }
    let path = CString::new(bytes)?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_beneath_with_mode(
    root_fd: RawFd,
    relative_path: &Path,
    flags: i32,
    mode: u32,
) -> io::Result<File> {
    validate_relative_path(relative_path)?;
    let path = if relative_path.as_os_str().is_empty() {
        CString::new(".")?
    } else {
        CString::new(relative_path.as_os_str().as_bytes())?
    };
    let mut how = unsafe { mem::zeroed::<libc::open_how>() };
    how.flags = (flags | libc::O_CLOEXEC) as u64;
    how.mode = mode as u64;
    how.resolve = libc::RESOLVE_IN_ROOT | libc::RESOLVE_NO_MAGICLINKS;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root_fd,
            path.as_ptr(),
            &how,
            mem::size_of::<libc::open_how>(),
        )
    } as RawFd;
    if fd >= 0 {
        return Ok(unsafe { File::from_raw_fd(fd) });
    }

    let error = io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        return Err(error);
    }

    let fd = unsafe { libc::openat(root_fd, path.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_flags_want_write(flags: u32) -> bool {
    let access_mode = flags as i32 & libc::O_ACCMODE;
    access_mode == libc::O_WRONLY
        || access_mode == libc::O_RDWR
        || flags & (libc::O_TRUNC as u32 | libc::O_APPEND as u32) != 0
}

fn fstat_file(file: &File) -> io::Result<fs::Metadata> {
    file.metadata()
}

fn parent_and_leaf(path: &Path) -> io::Result<(PathBuf, CString)> {
    validate_relative_path(path)?;
    let leaf = path
        .file_name()
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    if leaf.as_bytes().contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
    Ok((parent, CString::new(leaf.as_bytes())?))
}

fn with_parent_dir<T>(
    root_fd: RawFd,
    path: &Path,
    f: impl FnOnce(RawFd, &CStr) -> io::Result<T>,
) -> io::Result<T> {
    let (parent, leaf) = parent_and_leaf(path)?;
    let parent_dir =
        open_beneath_with_mode(root_fd, &parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    f(parent_dir.as_raw_fd(), leaf.as_c_str())
}

fn mkdir_beneath(root_fd: RawFd, path: &Path, mode: u32) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::mkdirat(parent_fd, leaf.as_ptr(), mode) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn mknod_beneath(root_fd: RawFd, path: &Path, mode: u32, rdev: u32) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::mknodat(parent_fd, leaf.as_ptr(), mode, rdev.into()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn unlink_beneath(root_fd: RawFd, path: &Path, directory: bool) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
        let result = unsafe { libc::unlinkat(parent_fd, leaf.as_ptr(), flags) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn rename_beneath(root_fd: RawFd, old_path: &Path, new_path: &Path, flags: u32) -> io::Result<()> {
    let (old_parent, old_leaf) = parent_and_leaf(old_path)?;
    let (new_parent, new_leaf) = parent_and_leaf(new_path)?;
    let old_parent =
        open_beneath_with_mode(root_fd, &old_parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    let new_parent =
        open_beneath_with_mode(root_fd, &new_parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    let result = if flags == 0 {
        unsafe {
            libc::renameat(
                old_parent.as_raw_fd(),
                old_leaf.as_ptr(),
                new_parent.as_raw_fd(),
                new_leaf.as_ptr(),
            )
        }
    } else {
        unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                old_parent.as_raw_fd(),
                old_leaf.as_ptr(),
                new_parent.as_raw_fd(),
                new_leaf.as_ptr(),
                flags,
            ) as i32
        }
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn link_beneath(root_fd: RawFd, old_path: &Path, new_path: &Path) -> io::Result<()> {
    let (old_parent, old_leaf) = parent_and_leaf(old_path)?;
    let (new_parent, new_leaf) = parent_and_leaf(new_path)?;
    let old_parent =
        open_beneath_with_mode(root_fd, &old_parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    let new_parent =
        open_beneath_with_mode(root_fd, &new_parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    let result = unsafe {
        libc::linkat(
            old_parent.as_raw_fd(),
            old_leaf.as_ptr(),
            new_parent.as_raw_fd(),
            new_leaf.as_ptr(),
            0,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn symlink_beneath(root_fd: RawFd, linkname: &CStr, path: &Path) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::symlinkat(linkname.as_ptr(), parent_fd, leaf.as_ptr()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn readlink_beneath(root_fd: RawFd, path: &Path) -> io::Result<Vec<u8>> {
    validate_relative_path(path)?;
    let c_path = if path.as_os_str().is_empty() {
        CString::new(".")?
    } else {
        CString::new(path.as_os_str().as_bytes())?
    };
    let mut buffer = vec![0; 4096];
    let result = unsafe {
        libc::readlinkat(
            root_fd,
            c_path.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(result as usize);
    Ok(buffer)
}

fn access_beneath(root_fd: RawFd, path: &Path, mask: i32) -> io::Result<()> {
    validate_relative_path(path)?;
    let c_path = if path.as_os_str().is_empty() {
        CString::new(".")?
    } else {
        CString::new(path.as_os_str().as_bytes())?
    };
    let result = unsafe { libc::faccessat(root_fd, c_path.as_ptr(), mask, libc::AT_EACCESS) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn access_host_path(mount: &MountRuntime, path: &Path, mask: i32) -> io::Result<()> {
    if path.as_os_str().is_empty() && matches!(mount.kind, MountKind::File) {
        let bytes = mount.root_path.as_os_str().as_bytes();
        if bytes.contains(&0) {
            return Err(io::Error::from_raw_os_error(libc::ENOENT));
        }
        let c_path = CString::new(bytes)?;
        let result =
            unsafe { libc::faccessat(libc::AT_FDCWD, c_path.as_ptr(), mask, libc::AT_EACCESS) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(());
    }
    access_beneath(mount.root.as_raw_fd(), path, mask)
}

fn fstatvfs_file(file: &File) -> io::Result<libc::statvfs64> {
    let mut st: libc::statvfs64 = unsafe { mem::zeroed() };
    let result = unsafe { libc::fstatvfs64(file.as_raw_fd(), &mut st) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

fn setattr_wants_mutation(valid: SetattrValid) -> bool {
    valid.intersects(
        SetattrValid::MODE
            | SetattrValid::UID
            | SetattrValid::GID
            | SetattrValid::SIZE
            | SetattrValid::ATIME
            | SetattrValid::MTIME
            | SetattrValid::ATIME_NOW
            | SetattrValid::MTIME_NOW,
    )
}

fn apply_setattr(file: &File, attr: fuse::SetattrIn, valid: SetattrValid) -> io::Result<()> {
    let fd = file.as_raw_fd();
    if valid.contains(SetattrValid::MODE) {
        let result = unsafe { libc::fchmod(fd, attr.mode & 0o7777) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if valid.intersects(SetattrValid::UID | SetattrValid::GID) {
        let uid = if valid.contains(SetattrValid::UID) {
            attr.uid.into_inner()
        } else {
            u32::MAX
        };
        let gid = if valid.contains(SetattrValid::GID) {
            attr.gid.into_inner()
        } else {
            u32::MAX
        };
        let result = unsafe { libc::fchown(fd, uid, gid) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if valid.contains(SetattrValid::SIZE) {
        let result = unsafe { libc::ftruncate64(fd, attr.size as i64) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if valid.intersects(
        SetattrValid::ATIME
            | SetattrValid::MTIME
            | SetattrValid::ATIME_NOW
            | SetattrValid::MTIME_NOW,
    ) {
        let times = [
            setattr_time(
                valid.contains(SetattrValid::ATIME),
                valid.contains(SetattrValid::ATIME_NOW),
                attr.atime,
                attr.atimensec,
            ),
            setattr_time(
                valid.contains(SetattrValid::MTIME),
                valid.contains(SetattrValid::MTIME_NOW),
                attr.mtime,
                attr.mtimensec,
            ),
        ];
        let result = unsafe { libc::futimens(fd, times.as_ptr()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn setxattr_file(file: &File, name: &CStr, value: &[u8], flags: u32) -> io::Result<()> {
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            flags as i32,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn getxattr_file(file: &File, name: &CStr, size: u32) -> io::Result<GetxattrReply> {
    let needed =
        unsafe { libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if needed < 0 {
        return Err(io::Error::last_os_error());
    }
    if size == 0 {
        return Ok(GetxattrReply::Count(needed as u32));
    }
    let mut value = vec![0; size as usize];
    let actual = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    value.truncate(actual as usize);
    Ok(GetxattrReply::Value(value))
}

fn listxattr_file(file: &File, size: u32) -> io::Result<ListxattrReply> {
    let needed = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
    if needed < 0 {
        return Err(io::Error::last_os_error());
    }
    if size == 0 {
        return Ok(ListxattrReply::Count(needed as u32));
    }
    let mut names = vec![0; size as usize];
    let actual =
        unsafe { libc::flistxattr(file.as_raw_fd(), names.as_mut_ptr().cast(), names.len()) };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    names.truncate(actual as usize);
    Ok(ListxattrReply::Names(names))
}

fn removexattr_file(file: &File, name: &CStr) -> io::Result<()> {
    let result = unsafe { libc::fremovexattr(file.as_raw_fd(), name.as_ptr()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn setattr_time(set_explicit: bool, set_now: bool, sec: u64, nsec: u32) -> libc::timespec {
    if set_now {
        return libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW,
        };
    }
    if set_explicit {
        return libc::timespec {
            tv_sec: sec as libc::time_t,
            tv_nsec: nsec as libc::c_long,
        };
    }
    libc::timespec {
        tv_sec: 0,
        tv_nsec: libc::UTIME_OMIT,
    }
}

struct HostDirEntry {
    name: String,
    metadata: fs::Metadata,
}

fn read_dir_beneath(root_fd: RawFd, relative_path: &Path) -> io::Result<Vec<HostDirEntry>> {
    let dir = open_beneath(
        root_fd,
        relative_path,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )?;
    let proc_path = format!("/proc/self/fd/{}", dir.as_raw_fd());
    let mut entries = Vec::new();
    for entry in fs::read_dir(proc_path)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if validate_single_path_component(name).is_err() {
            continue;
        }
        entries.push(HostDirEntry {
            name: name.to_string(),
            metadata: fs::symlink_metadata(entry.path())?,
        });
    }
    Ok(entries)
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

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config, FileFailurePersistence, TestRunner};
    use std::os::unix::fs::FileExt;
    use std::thread;
    use virtiofsd::oslib::{ReadvFlags, WritevFlags};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agentvm-composed-fs-{name}-{}-{}",
                std::process::id(),
                now_secs()
            ));
            fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn ctx() -> Context {
        Context {
            uid: GuestUid::from(0),
            gid: GuestGid::from(0),
            pid: 0,
        }
    }

    fn manifest_with_mounts(mounts: Vec<MountSpec>) -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            export_tag: Some(DEFAULT_TAG.to_string()),
            created_by: Some("test".to_string()),
            mounts,
            synthetic: Some(SyntheticSpec {
                uid: 0,
                gid: 0,
                dir_mode: "0555".to_string(),
            }),
            protected_guest_paths: Vec::new(),
        }
    }

    fn dir_mount(id: &str, guest_path: &str, host_path: &Path, access: AccessMode) -> MountSpec {
        MountSpec {
            id: id.to_string(),
            guest_path: guest_path.to_string(),
            host_path: host_path.display().to_string(),
            kind: MountKind::Dir,
            access,
            source_class: SourceClass::Workspace,
            required: true,
            bind: true,
            metadata: MetadataSpec {
                uid_gid: MetadataPolicy::Host,
                permissions: MetadataPolicy::Host,
            },
        }
    }

    fn file_mount(id: &str, guest_path: &str, host_path: &Path, access: AccessMode) -> MountSpec {
        MountSpec {
            id: id.to_string(),
            guest_path: guest_path.to_string(),
            host_path: host_path.display().to_string(),
            kind: MountKind::File,
            access,
            source_class: SourceClass::SystemRo,
            required: true,
            bind: true,
            metadata: MetadataSpec {
                uid_gid: MetadataPolicy::Host,
                permissions: MetadataPolicy::Host,
            },
        }
    }

    fn lookup(fs: &ComposedFs, parent: u64, name: &str) -> io::Result<Entry> {
        let name = CString::new(name).expect("test name");
        fs.lookup(ctx(), parent, name.as_c_str())
    }

    fn raw_error<T>(result: io::Result<T>, label: &str) -> Option<i32> {
        match result {
            Ok(_) => panic!("{label} succeeded unexpectedly"),
            Err(error) => error.raw_os_error(),
        }
    }

    fn lookup_count(fs: &ComposedFs, inode: u64) -> u64 {
        fs.namespace
            .read()
            .expect("namespace lock")
            .nodes
            .get(&inode)
            .expect("inode")
            .lookup_count
    }

    fn dir_names(mut iter: VecDirIter) -> Vec<String> {
        let mut names = Vec::new();
        while let Some(entry) = iter.next() {
            names.push(entry.name.to_str().expect("utf8").to_string());
        }
        names
    }

    #[derive(Clone)]
    struct TestRng(u64);

    impl TestRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        fn usize(&mut self, upper: usize) -> usize {
            if upper == 0 {
                0
            } else {
                (self.next_u64() as usize) % upper
            }
        }
    }

    fn traced<T>(result: io::Result<T>, seed: u64, ops: &[String], label: &str) -> T {
        result.unwrap_or_else(|error| {
            panic!(
                "{label} failed for seed {seed:#x}: {error}\noperation trace:\n{}",
                ops.join("\n")
            )
        })
    }

    fn traced_ops<T>(result: io::Result<T>, ops: &[String], label: &str) -> T {
        result.unwrap_or_else(|error| {
            panic!(
                "{label} failed: {error}\noperation trace:\n{}",
                ops.join("\n")
            )
        })
    }

    #[derive(Clone, Debug)]
    enum FsStressOp {
        Put { name: usize, len: usize, byte: u8 },
        Read { name: usize },
        Rename { from: usize, to: usize },
        Unlink { name: usize },
        HostPut { name: usize, len: usize, byte: u8 },
        Readdir,
    }

    fn fs_stress_ops() -> impl Strategy<Value = Vec<FsStressOp>> {
        prop::collection::vec(
            prop_oneof![
                (0usize..48, 0usize..128, any::<u8>()).prop_map(|(name, len, byte)| {
                    FsStressOp::Put {
                        name,
                        len: len + 1,
                        byte,
                    }
                }),
                (0usize..48).prop_map(|name| FsStressOp::Read { name }),
                (0usize..48, 0usize..48).prop_map(|(from, to)| FsStressOp::Rename { from, to }),
                (0usize..48).prop_map(|name| FsStressOp::Unlink { name }),
                (0usize..48, 0usize..128, any::<u8>()).prop_map(|(name, len, byte)| {
                    FsStressOp::HostPut {
                        name,
                        len: len + 1,
                        byte,
                    }
                }),
                Just(FsStressOp::Readdir),
            ],
            1..160,
        )
    }

    fn stress_name(index: usize) -> String {
        format!("p{index:03}.txt")
    }

    fn run_flat_file_ops_case(case_name: &str, generated_ops: &[FsStressOp]) {
        let test_dir = TestDir::new(case_name);
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let mut model = BTreeMap::<String, Vec<u8>>::new();
        let mut trace = Vec::<String>::new();

        for (step, op) in generated_ops.iter().enumerate() {
            match op {
                FsStressOp::Put { name, len, byte } => {
                    let name = stress_name(*name);
                    let data = vec![*byte; *len];
                    trace.push(format!("{step}: put {name} len={len} byte={byte}"));
                    let c_name = CString::new(name.as_str()).expect("name");
                    let (entry, handle, _options) = traced_ops(
                        fs.create(
                            ctx(),
                            workspace.inode,
                            c_name.as_c_str(),
                            0o644,
                            false,
                            (libc::O_RDWR | libc::O_TRUNC) as u32,
                            0,
                            Extensions::default(),
                        ),
                        &trace,
                        "create",
                    );
                    let handle = handle.expect("handle");
                    traced_ops(
                        fs.write(
                            ctx(),
                            entry.inode,
                            handle,
                            VecReader { data: data.clone() },
                            data.len() as u32,
                            0,
                            None,
                            false,
                            false,
                            0,
                        ),
                        &trace,
                        "write",
                    );
                    traced_ops(
                        fs.release(ctx(), entry.inode, 0, handle, true, false, None),
                        &trace,
                        "release",
                    );
                    model.insert(name, data);
                }
                FsStressOp::Read { name } => {
                    let name = stress_name(*name);
                    trace.push(format!("{step}: read {name}"));
                    let Some(expected) = model.get(&name).cloned() else {
                        assert_eq!(
                            raw_error(lookup(&fs, workspace.inode, &name), "missing read lookup"),
                            Some(libc::ENOENT),
                            "operation trace:\n{}",
                            trace.join("\n")
                        );
                        continue;
                    };
                    let entry = traced_ops(lookup(&fs, workspace.inode, &name), &trace, "lookup");
                    let (handle, _options) = traced_ops(
                        fs.open(ctx(), entry.inode, false, libc::O_RDONLY as u32),
                        &trace,
                        "open read",
                    );
                    let handle = handle.expect("handle");
                    let mut reader = VecWriter::default();
                    traced_ops(
                        fs.read(
                            ctx(),
                            entry.inode,
                            handle,
                            &mut reader,
                            (expected.len() + 16) as u32,
                            0,
                            None,
                            0,
                        ),
                        &trace,
                        "read",
                    );
                    assert_eq!(
                        reader.data,
                        expected,
                        "read mismatch\noperation trace:\n{}",
                        trace.join("\n")
                    );
                    traced_ops(
                        fs.release(ctx(), entry.inode, 0, handle, false, false, None),
                        &trace,
                        "release read",
                    );
                    fs.forget(ctx(), entry.inode, 1);
                }
                FsStressOp::Rename { from, to } => {
                    let from = stress_name(*from);
                    let to = stress_name(*to);
                    trace.push(format!("{step}: rename {from} -> {to}"));
                    let old_c = CString::new(from.as_str()).expect("old");
                    let new_c = CString::new(to.as_str()).expect("new");
                    if !model.contains_key(&from) {
                        assert_eq!(
                            raw_error(
                                fs.rename(
                                    ctx(),
                                    workspace.inode,
                                    old_c.as_c_str(),
                                    workspace.inode,
                                    new_c.as_c_str(),
                                    0,
                                ),
                                "missing rename",
                            ),
                            Some(libc::ENOENT),
                            "operation trace:\n{}",
                            trace.join("\n")
                        );
                        continue;
                    }
                    traced_ops(
                        fs.rename(
                            ctx(),
                            workspace.inode,
                            old_c.as_c_str(),
                            workspace.inode,
                            new_c.as_c_str(),
                            0,
                        ),
                        &trace,
                        "rename",
                    );
                    if from != to {
                        let data = model.remove(&from).expect("from");
                        model.insert(to, data);
                    }
                }
                FsStressOp::Unlink { name } => {
                    let name = stress_name(*name);
                    trace.push(format!("{step}: unlink {name}"));
                    let c_name = CString::new(name.as_str()).expect("name");
                    let result = fs.unlink(ctx(), workspace.inode, c_name.as_c_str());
                    if model.remove(&name).is_some() {
                        traced_ops(result, &trace, "unlink");
                    } else {
                        assert_eq!(
                            raw_error(result, "missing unlink"),
                            Some(libc::ENOENT),
                            "operation trace:\n{}",
                            trace.join("\n")
                        );
                    }
                }
                FsStressOp::HostPut { name, len, byte } => {
                    let name = stress_name(*name);
                    let data = vec![*byte; *len];
                    trace.push(format!("{step}: host-put {name} len={len} byte={byte}"));
                    fs::write(root.join(&name), &data).unwrap_or_else(|error| {
                        panic!(
                            "host put failed: {error}\noperation trace:\n{}",
                            trace.join("\n")
                        )
                    });
                    model.insert(name, data);
                }
                FsStressOp::Readdir => {
                    trace.push(format!("{step}: readdir"));
                    let mut names = dir_names(traced_ops(
                        fs.readdir(ctx(), workspace.inode, workspace.inode, 16 * 1024, 0),
                        &trace,
                        "readdir",
                    ));
                    names.sort();
                    let expected = model.keys().cloned().collect::<Vec<_>>();
                    assert_eq!(
                        names,
                        expected,
                        "readdir mismatch\noperation trace:\n{}",
                        trace.join("\n")
                    );
                }
            }
        }
    }

    fn assert_invalid_input<T>(result: io::Result<T>, message: &str) {
        let error = match result {
            Ok(_) => panic!("{message} succeeded unexpectedly"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
        assert!(
            error.to_string().contains(message),
            "expected error containing {message:?}, got {error}"
        );
    }

    struct VecReader {
        data: Vec<u8>,
    }

    impl ZeroCopyReader for VecReader {
        fn write_to_file_at(
            &mut self,
            file: &File,
            count: usize,
            offset: u64,
            _flags: Option<WritevFlags>,
        ) -> io::Result<usize> {
            let count = count.min(self.data.len());
            file.write_at(&self.data[..count], offset)
        }
    }

    #[derive(Default)]
    struct VecWriter {
        data: Vec<u8>,
    }

    impl ZeroCopyWriter for VecWriter {
        fn read_from_file_at(
            &mut self,
            file: &File,
            count: usize,
            offset: u64,
            _flags: Option<ReadvFlags>,
        ) -> io::Result<usize> {
            let start = self.data.len();
            self.data.resize(start + count, 0);
            let read = file.read_at(&mut self.data[start..], offset)?;
            self.data.truncate(start + read);
            Ok(read)
        }
    }

    #[test]
    fn shared_guest_share_fixture_builds_expected_mounts() {
        let fixture = test_support::GuestShareFixture::new("shared-fixture");
        let fs = fixture.filesystem();

        let workspace = test_support::lookup_root(&fs, "workspace").expect("lookup workspace");
        let project =
            test_support::lookup(&fs, workspace.inode, "project.txt").expect("lookup project");
        let (handle, _options) = fs
            .open(ctx(), project.inode, false, libc::O_RDONLY as u32)
            .expect("open project file");
        let mut reader = test_support::VecWriter::default();
        fs.read(
            ctx(),
            project.inode,
            handle.expect("handle"),
            &mut reader,
            64,
            0,
            None,
            0,
        )
        .expect("read project file");

        assert_eq!(reader.data, b"workspace");
        assert!(fixture.root.path().exists());
    }

    #[test]
    fn manifest_validation_rejects_schema_empty_duplicate_and_protected_paths() {
        let fixture = test_support::GuestShareFixture::new("manifest-validation");

        let mut bad_schema = fixture.manifest();
        bad_schema.schema_version = 999;
        assert_invalid_input(
            Namespace::from_manifest(&bad_schema),
            "unsupported schema_version",
        );

        let empty = test_support::manifest_with_mounts(Vec::new());
        assert_invalid_input(
            Namespace::from_manifest(&empty),
            "manifest must contain at least one mount",
        );

        let duplicate = test_support::manifest_with_mounts(vec![
            test_support::dir_mount(
                "left",
                "/workspace",
                &fixture.workspace,
                AccessMode::Rw,
                SourceClass::Workspace,
            ),
            test_support::dir_mount(
                "right",
                "/workspace",
                &fixture.readonly,
                AccessMode::Ro,
                SourceClass::UserRo,
            ),
        ]);
        assert_invalid_input(
            Namespace::from_manifest(&duplicate),
            "duplicate guest_path in manifest",
        );

        let mut protected = fixture.manifest();
        protected.protected_guest_paths = vec!["/workspace".to_string()];
        assert_invalid_input(
            Namespace::from_manifest(&protected),
            "mount targets protected guest path",
        );
    }

    #[test]
    fn manifest_validation_rejects_relative_dotdot_and_missing_sources() {
        let fixture = test_support::GuestShareFixture::new("manifest-paths");

        let relative_host = test_support::manifest_with_mounts(vec![MountSpec {
            id: "relative-host".to_string(),
            guest_path: "/workspace".to_string(),
            host_path: "relative".to_string(),
            kind: MountKind::Dir,
            access: AccessMode::Rw,
            source_class: SourceClass::Workspace,
            required: true,
            bind: true,
            metadata: MetadataSpec {
                uid_gid: MetadataPolicy::Host,
                permissions: MetadataPolicy::Host,
            },
        }]);
        assert_invalid_input(
            Namespace::from_manifest(&relative_host),
            "host_path must be absolute",
        );

        let dotdot_guest = test_support::manifest_with_mounts(vec![test_support::dir_mount(
            "dotdot",
            "/workspace/../escape",
            &fixture.workspace,
            AccessMode::Rw,
            SourceClass::Workspace,
        )]);
        assert_invalid_input(
            Namespace::from_manifest(&dotdot_guest),
            "guest_path contains invalid component",
        );

        let missing = test_support::manifest_with_mounts(vec![test_support::dir_mount(
            "missing",
            "/missing",
            &fixture.root.join("missing"),
            AccessMode::Rw,
            SourceClass::Workspace,
        )]);
        assert!(matches!(
            Namespace::from_manifest(&missing)
                .expect_err("missing source")
                .kind(),
            io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn operation_model_sequence_matches_host_tree_for_rw_mount() {
        let fixture = test_support::GuestShareFixture::new("operation-model-rw");
        let fs = fixture.filesystem();
        let mut ops = Vec::new();
        macro_rules! step {
            ($label:expr, $expr:expr) => {{
                ops.push($label);
                match $expr {
                    Ok(value) => value,
                    Err(error) => panic!(
                        "operation {:?} failed: {}; sequence: {:?}",
                        $label, error, ops
                    ),
                }
            }};
        }

        let workspace = step!(
            "lookup /workspace",
            test_support::lookup_root(&fs, "workspace")
        );
        let missing = raw_error(
            test_support::lookup(&fs, workspace.inode, "missing"),
            "lookup missing",
        );
        assert_eq!(missing, Some(libc::ENOENT), "sequence: {ops:?}");

        let dir_name = CString::new("model-dir").expect("dir name");
        let dir = step!(
            "mkdir /workspace/model-dir",
            fs.mkdir(
                ctx(),
                workspace.inode,
                dir_name.as_c_str(),
                0o755,
                0,
                Extensions::default(),
            )
        );
        assert!(fixture.workspace.join("model-dir").is_dir());

        let file_name = CString::new("data.bin").expect("file name");
        let (file, handle, _options) = step!(
            "create /workspace/model-dir/data.bin",
            fs.create(
                ctx(),
                dir.inode,
                file_name.as_c_str(),
                0o644,
                false,
                (libc::O_RDWR | libc::O_TRUNC) as u32,
                0,
                Extensions::default(),
            )
        );
        let handle = handle.expect("file handle");
        let initial = b"\0abc\xffdef";
        let written = step!(
            "write initial binary content",
            fs.write(
                ctx(),
                file.inode,
                handle,
                test_support::VecReader {
                    data: initial.to_vec()
                },
                initial.len() as u32,
                0,
                None,
                false,
                false,
                0,
            )
        );
        assert_eq!(written, initial.len());
        assert_eq!(
            fs::read(fixture.workspace.join("model-dir/data.bin")).expect("host read"),
            initial
        );

        let mut reader = test_support::VecWriter::default();
        let read = step!(
            "read offset slice",
            fs.read(ctx(), file.inode, handle, &mut reader, 4, 2, None, 0)
        );
        assert_eq!(read, 4);
        assert_eq!(reader.data, b"bc\xffd");

        let append = b"-tail";
        step!(
            "append content at explicit EOF offset",
            fs.write(
                ctx(),
                file.inode,
                handle,
                test_support::VecReader {
                    data: append.to_vec()
                },
                append.len() as u32,
                initial.len() as u64,
                None,
                false,
                false,
                0,
            )
        );
        assert_eq!(
            fs::read(fixture.workspace.join("model-dir/data.bin")).expect("host read append"),
            [initial.as_slice(), append.as_slice()].concat()
        );

        let mut attr = fuse::SetattrIn::default();
        attr.size = 5;
        step!(
            "truncate file to five bytes",
            fs.setattr(ctx(), file.inode, attr, None, SetattrValid::SIZE)
        );
        assert_eq!(
            fs::read(fixture.workspace.join("model-dir/data.bin")).expect("host read truncate"),
            b"\0abc\xff"
        );
        step!(
            "flush writable handle",
            fs.flush(ctx(), file.inode, handle, 0)
        );
        step!(
            "fsync writable handle",
            fs.fsync(ctx(), file.inode, false, handle)
        );

        fs::write(fixture.workspace.join("host-created.txt"), b"host-side").expect("host mutation");
        let host_created = step!(
            "lookup host-created file",
            test_support::lookup(&fs, workspace.inode, "host-created.txt")
        );
        let (host_handle, _options) = step!(
            "open host-created file",
            fs.open(ctx(), host_created.inode, false, libc::O_RDONLY as u32)
        );
        let mut host_reader = test_support::VecWriter::default();
        step!(
            "read host-created file",
            fs.read(
                ctx(),
                host_created.inode,
                host_handle.expect("host handle"),
                &mut host_reader,
                64,
                0,
                None,
                0,
            )
        );
        assert_eq!(host_reader.data, b"host-side");

        let mut names = Vec::new();
        let mut iter = step!(
            "readdir workspace",
            fs.readdir(ctx(), workspace.inode, workspace.inode, 4096, 0)
        );
        while let Some(entry) = iter.next() {
            names.push(entry.name.to_str().expect("utf8").to_string());
        }
        names.sort();
        assert!(names.contains(&"host-created.txt".to_string()));
        assert!(names.contains(&"model-dir".to_string()));
        assert!(names.contains(&"project.txt".to_string()));

        let renamed = CString::new("renamed.bin").expect("renamed");
        step!(
            "rename data.bin to renamed.bin",
            fs.rename(
                ctx(),
                dir.inode,
                file_name.as_c_str(),
                dir.inode,
                renamed.as_c_str(),
                0,
            )
        );
        assert!(!fixture.workspace.join("model-dir/data.bin").exists());
        assert!(fixture.workspace.join("model-dir/renamed.bin").exists());

        step!(
            "unlink renamed file",
            fs.unlink(ctx(), dir.inode, renamed.as_c_str())
        );
        assert!(!fixture.workspace.join("model-dir/renamed.bin").exists());
        step!(
            "rmdir empty model-dir",
            fs.rmdir(ctx(), workspace.inode, dir_name.as_c_str())
        );
        assert!(!fixture.workspace.join("model-dir").exists());
    }

    #[test]
    fn operation_model_error_sequence_covers_ro_system_and_boundaries() {
        let fixture = test_support::GuestShareFixture::new("operation-model-errors");
        let fs = fixture.filesystem();
        let mut ops = Vec::new();
        macro_rules! step {
            ($label:expr, $expr:expr) => {{
                ops.push($label);
                match $expr {
                    Ok(value) => value,
                    Err(error) => panic!(
                        "operation {:?} failed: {}; sequence: {:?}",
                        $label, error, ops
                    ),
                }
            }};
        }
        macro_rules! expect_errno {
            ($label:expr, $expr:expr, $errno:expr) => {{
                ops.push($label);
                match $expr {
                    Ok(_) => panic!(
                        "operation {:?} succeeded unexpectedly; sequence: {:?}",
                        $label, ops
                    ),
                    Err(error) => assert_eq!(
                        error.raw_os_error(),
                        Some($errno),
                        "operation {:?}; sequence: {:?}; error: {}",
                        $label,
                        ops,
                        error
                    ),
                }
            }};
        }

        let workspace = step!(
            "lookup /workspace",
            test_support::lookup_root(&fs, "workspace")
        );
        let readonly = step!(
            "lookup /readonly",
            test_support::lookup_root(&fs, "readonly")
        );
        let run = step!("lookup /run", test_support::lookup_root(&fs, "run"));
        let config = step!(
            "lookup /run/agentvm-config",
            test_support::lookup(&fs, run.inode, "agentvm-config")
        );

        let blocked = CString::new("blocked.txt").expect("blocked");
        expect_errno!(
            "create in user readonly mount",
            fs.create(
                ctx(),
                readonly.inode,
                blocked.as_c_str(),
                0o644,
                false,
                libc::O_RDWR as u32,
                0,
                Extensions::default(),
            ),
            libc::EROFS
        );
        expect_errno!(
            "create in system config mount",
            fs.create(
                ctx(),
                config.inode,
                blocked.as_c_str(),
                0o644,
                false,
                libc::O_RDWR as u32,
                0,
                Extensions::default(),
            ),
            libc::EROFS
        );
        expect_errno!(
            "open directory as file",
            fs.open(ctx(), workspace.inode, false, libc::O_RDONLY as u32),
            libc::EISDIR
        );
        expect_errno!(
            "unsafe lookup component",
            test_support::lookup(&fs, workspace.inode, ".."),
            libc::ENOENT
        );

        let project = step!(
            "lookup project.txt",
            test_support::lookup(&fs, workspace.inode, "project.txt")
        );
        let moved = CString::new("moved.txt").expect("moved");
        let project_name = CString::new("project.txt").expect("project");
        expect_errno!(
            "cross-mount link into readonly",
            fs.link(ctx(), project.inode, readonly.inode, moved.as_c_str()),
            libc::EXDEV
        );
        expect_errno!(
            "cross-mount rename into readonly",
            fs.rename(
                ctx(),
                workspace.inode,
                project_name.as_c_str(),
                readonly.inode,
                moved.as_c_str(),
                0,
            ),
            libc::EXDEV
        );

        let nonempty = CString::new("nonempty").expect("nonempty");
        let child = CString::new("child").expect("child");
        let dir = step!(
            "mkdir nonempty",
            fs.mkdir(
                ctx(),
                workspace.inode,
                nonempty.as_c_str(),
                0o755,
                0,
                Extensions::default(),
            )
        );
        step!(
            "create child in nonempty",
            fs.create(
                ctx(),
                dir.inode,
                child.as_c_str(),
                0o644,
                false,
                libc::O_RDWR as u32,
                0,
                Extensions::default(),
            )
        );
        expect_errno!(
            "rmdir nonempty directory",
            fs.rmdir(ctx(), workspace.inode, nonempty.as_c_str()),
            libc::ENOTEMPTY
        );
    }

    #[test]
    fn host_lookup_reuses_dev_ino_and_tracks_forget() {
        let test_dir = TestDir::new("hardlink");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"hello").expect("write file");
        fs::hard_link(root.join("file"), root.join("link")).expect("create hardlink");

        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup mount");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let link = lookup(&fs, workspace.inode, "link").expect("lookup link");

        assert_eq!(file.inode, link.inode);
        {
            let namespace = fs.namespace.read().expect("namespace");
            let node = namespace.nodes.get(&file.inode).expect("file node");
            assert_eq!(node.lookup_count, 2);
        }
        fs.forget(ctx(), file.inode, 1);
        let namespace = fs.namespace.read().expect("namespace");
        let node = namespace.nodes.get(&file.inode).expect("file node");
        assert_eq!(node.lookup_count, 1);
    }

    #[test]
    fn nested_overlay_readdir_merges_host_entries_and_mount_boundary() {
        let test_dir = TestDir::new("overlay");
        let root = test_dir.path.join("root");
        let gh = test_dir.path.join("gh");
        fs::create_dir_all(root.join(".config")).expect("create config");
        fs::create_dir(&gh).expect("create gh");
        fs::write(root.join(".config").join("other"), b"host").expect("write host entry");
        fs::write(gh.join("hosts.yml"), b"token: nope").expect("write gh entry");

        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![
            dir_mount("home", "/home/user", &root, AccessMode::Rw),
            dir_mount("gh", "/home/user/.config/gh", &gh, AccessMode::Ro),
        ]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let home = lookup(&fs, ROOT_ID, "home").expect("lookup home");
        let user = lookup(&fs, home.inode, "user").expect("lookup user");
        let config = lookup(&fs, user.inode, ".config").expect("lookup config");
        let mut iter = fs
            .readdir(ctx(), config.inode, config.inode, 4096, 0)
            .expect("readdir config");
        let mut names = Vec::new();
        while let Some(entry) = iter.next() {
            names.push(entry.name.to_str().expect("utf8").to_string());
        }
        names.sort();

        assert_eq!(names, vec!["gh".to_string(), "other".to_string()]);
    }

    #[test]
    fn lookup_rejects_unsafe_components() {
        let test_dir = TestDir::new("unsafe");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup mount");

        let error = match lookup(&fs, workspace.inode, "..") {
            Ok(_) => panic!("unsafe name accepted"),
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
    }

    #[test]
    fn create_write_read_and_release_host_file() {
        let test_dir = TestDir::new("io");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let name = CString::new("created.txt").expect("name");
        let (entry, handle, _options) = fs
            .create(
                ctx(),
                workspace.inode,
                name.as_c_str(),
                0o644,
                false,
                (libc::O_RDWR | libc::O_TRUNC) as u32,
                0,
                Extensions::default(),
            )
            .expect("create file");
        let handle = handle.expect("file handle");

        let written = fs
            .write(
                ctx(),
                entry.inode,
                handle,
                VecReader {
                    data: b"hello composed fs".to_vec(),
                },
                17,
                0,
                None,
                false,
                false,
                0,
            )
            .expect("write file");
        assert_eq!(written, 17);

        let mut reader = VecWriter::default();
        let read = fs
            .read(ctx(), entry.inode, handle, &mut reader, 64, 0, None, 0)
            .expect("read file");
        assert_eq!(read, 17);
        assert_eq!(reader.data, b"hello composed fs");

        fs.release(ctx(), entry.inode, 0, handle, true, false, None)
            .expect("release file");
        assert_eq!(
            fs::read_to_string(root.join("created.txt")).expect("read host file"),
            "hello composed fs"
        );
    }

    #[test]
    fn readonly_mount_rejects_create() {
        let test_dir = TestDir::new("readonly");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "readonly",
            "/readonly",
            &root,
            AccessMode::Ro,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let readonly = lookup(&fs, ROOT_ID, "readonly").expect("lookup mount");
        let name = CString::new("blocked").expect("name");
        let error = match fs.create(
            ctx(),
            readonly.inode,
            name.as_c_str(),
            0o644,
            false,
            libc::O_RDWR as u32,
            0,
            Extensions::default(),
        ) {
            Ok(_) => panic!("readonly create succeeded"),
            Err(error) => error,
        };

        assert_eq!(error.raw_os_error(), Some(libc::EROFS));
        assert!(!root.join("blocked").exists());
    }

    #[test]
    fn directory_link_rename_and_symlink_operations_use_host_tree() {
        let test_dir = TestDir::new("mutations");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("source"), b"data").expect("write source");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

        let dir_name = CString::new("dir").expect("dir name");
        fs.mkdir(
            ctx(),
            workspace.inode,
            dir_name.as_c_str(),
            0o755,
            0,
            Extensions::default(),
        )
        .expect("mkdir");
        assert!(root.join("dir").is_dir());

        let source = lookup(&fs, workspace.inode, "source").expect("lookup source");
        let link_name = CString::new("linked").expect("link name");
        fs.link(ctx(), source.inode, workspace.inode, link_name.as_c_str())
            .expect("link");
        assert_eq!(
            fs::metadata(root.join("source"))
                .expect("source metadata")
                .ino(),
            fs::metadata(root.join("linked"))
                .expect("link metadata")
                .ino()
        );

        let old_name = CString::new("linked").expect("old name");
        let new_name = CString::new("renamed").expect("new name");
        fs.rename(
            ctx(),
            workspace.inode,
            old_name.as_c_str(),
            workspace.inode,
            new_name.as_c_str(),
            0,
        )
        .expect("rename");
        assert!(!root.join("linked").exists());
        assert!(root.join("renamed").exists());

        let target = CString::new("renamed").expect("target");
        let symlink_name = CString::new("sym").expect("symlink name");
        let sym = fs
            .symlink(
                ctx(),
                target.as_c_str(),
                workspace.inode,
                symlink_name.as_c_str(),
                Extensions::default(),
            )
            .expect("symlink");
        assert_eq!(fs.readlink(ctx(), sym.inode).expect("readlink"), b"renamed");
    }

    #[test]
    fn readonly_mount_rejects_write_access_probe() {
        let test_dir = TestDir::new("readonly-access");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "readonly",
            "/readonly",
            &root,
            AccessMode::Ro,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let readonly = lookup(&fs, ROOT_ID, "readonly").expect("lookup mount");
        let file = lookup(&fs, readonly.inode, "file").expect("lookup file");

        fs.access(ctx(), file.inode, libc::R_OK as u32)
            .expect("read access");
        let error = match fs.access(ctx(), file.inode, libc::W_OK as u32) {
            Ok(_) => panic!("readonly write access succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(libc::EROFS));
    }

    #[test]
    fn mknod_supports_regular_fallback_and_rejects_special_nodes() {
        let test_dir = TestDir::new("mknod");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

        let regular = CString::new("regular").expect("regular");
        fs.mknod(
            ctx(),
            workspace.inode,
            regular.as_c_str(),
            libc::S_IFREG | 0o644,
            0,
            0,
            Extensions::default(),
        )
        .expect("regular mknod");
        assert!(root.join("regular").is_file());

        let special = CString::new("special").expect("special");
        let error = match fs.mknod(
            ctx(),
            workspace.inode,
            special.as_c_str(),
            libc::S_IFCHR | 0o644,
            0,
            0,
            Extensions::default(),
        ) {
            Ok(_) => panic!("special mknod succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn setattr_truncates_and_changes_mode_on_writable_mount() {
        let test_dir = TestDir::new("setattr");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"abcdef").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");

        let mut attr = fuse::SetattrIn::default();
        attr.size = 3;
        attr.mode = 0o600;
        fs.setattr(
            ctx(),
            file.inode,
            attr,
            None,
            SetattrValid::SIZE | SetattrValid::MODE,
        )
        .expect("setattr");

        assert_eq!(
            fs::read_to_string(root.join("file")).expect("read file"),
            "abc"
        );
        assert_eq!(
            fs::metadata(root.join("file")).expect("metadata").mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn readonly_mount_rejects_mutating_setattr() {
        let test_dir = TestDir::new("readonly-setattr");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"abcdef").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "readonly",
            "/readonly",
            &root,
            AccessMode::Ro,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let readonly = lookup(&fs, ROOT_ID, "readonly").expect("lookup readonly");
        let file = lookup(&fs, readonly.inode, "file").expect("lookup file");

        let mut attr = fuse::SetattrIn::default();
        attr.size = 3;
        let error = match fs.setattr(ctx(), file.inode, attr, None, SetattrValid::SIZE) {
            Ok(_) => panic!("readonly setattr succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(libc::EROFS));
        assert_eq!(
            fs::read_to_string(root.join("file")).expect("read file"),
            "abcdef"
        );
    }

    #[test]
    fn xattrs_delegate_to_host_when_supported() {
        let test_dir = TestDir::new("xattr");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let name = CString::new("user.agentvm_test").expect("xattr name");

        if let Err(error) = fs.setxattr(
            ctx(),
            file.inode,
            name.as_c_str(),
            b"value",
            0,
            SetxattrFlags::empty(),
        ) {
            if error.raw_os_error() == Some(libc::EOPNOTSUPP) {
                return;
            }
            panic!("setxattr failed unexpectedly: {error}");
        }

        match fs
            .getxattr(ctx(), file.inode, name.as_c_str(), 0)
            .expect("getxattr count")
        {
            GetxattrReply::Count(count) => assert_eq!(count, 5),
            GetxattrReply::Value(_) => panic!("expected count"),
        }
        match fs
            .getxattr(ctx(), file.inode, name.as_c_str(), 64)
            .expect("getxattr value")
        {
            GetxattrReply::Value(value) => assert_eq!(value, b"value"),
            GetxattrReply::Count(_) => panic!("expected value"),
        }
        match fs.listxattr(ctx(), file.inode, 4096).expect("listxattr") {
            ListxattrReply::Names(names) => {
                assert!(names
                    .split(|byte| *byte == 0)
                    .any(|entry| entry == b"user.agentvm_test"));
            }
            ListxattrReply::Count(_) => panic!("expected names"),
        }
        fs.removexattr(ctx(), file.inode, name.as_c_str())
            .expect("removexattr");
    }

    #[test]
    fn getattr_uses_cached_metadata_after_unlink() {
        let test_dir = TestDir::new("stale");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let name = CString::new("file").expect("name");

        fs.unlink(ctx(), workspace.inode, name.as_c_str())
            .expect("unlink");
        assert!(!root.join("file").exists());
        let (attr, _ttl) = fs
            .getattr(ctx(), file.inode, None)
            .expect("cached getattr after unlink");
        assert_eq!(attr.ino, file.inode);
        assert_eq!(attr.size, 4);
    }

    #[test]
    fn file_mount_opens_as_regular_file() {
        let test_dir = TestDir::new("file-mount");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let mounted_file = root.join("hosts");
        fs::write(&mounted_file, b"127.0.0.1 localhost\n").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![file_mount(
            "hosts",
            "/etc/hosts",
            &mounted_file,
            AccessMode::Ro,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let etc = lookup(&fs, ROOT_ID, "etc").expect("lookup etc");
        let hosts = lookup(&fs, etc.inode, "hosts").expect("lookup hosts");
        let (handle, _options) = fs
            .open(ctx(), hosts.inode, false, libc::O_RDONLY as u32)
            .expect("open file mount");
        let handle = handle.expect("handle");
        let mut reader = VecWriter::default();
        fs.read(ctx(), hosts.inode, handle, &mut reader, 64, 0, None, 0)
            .expect("read file mount");
        assert_eq!(reader.data, b"127.0.0.1 localhost\n");
    }

    #[test]
    fn symlink_escape_does_not_open_outside_mount_root() {
        let test_dir = TestDir::new("symlink-escape");
        let root = test_dir.path.join("root");
        let outside = test_dir.path.join("outside");
        fs::create_dir(&root).expect("create root");
        fs::create_dir(&outside).expect("create outside");
        fs::write(outside.join("secret"), b"secret").expect("write secret");
        std::os::unix::fs::symlink(outside.join("secret"), root.join("escape"))
            .expect("create symlink");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let escape = lookup(&fs, workspace.inode, "escape").expect("lookup symlink");

        let error = match fs.open(ctx(), escape.inode, false, libc::O_RDONLY as u32) {
            Ok(_) => panic!("opened symlink escape"),
            Err(error) => error,
        };
        assert!(matches!(
            error.raw_os_error(),
            Some(libc::ENOENT) | Some(libc::EXDEV) | Some(libc::ELOOP)
        ));
    }

    #[test]
    fn cross_mount_rename_and_link_return_exdev() {
        let test_dir = TestDir::new("cross-mount");
        let left = test_dir.path.join("left");
        let right = test_dir.path.join("right");
        fs::create_dir(&left).expect("create left");
        fs::create_dir(&right).expect("create right");
        fs::write(left.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![
            dir_mount("left", "/left", &left, AccessMode::Rw),
            dir_mount("right", "/right", &right, AccessMode::Rw),
        ]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let left_node = lookup(&fs, ROOT_ID, "left").expect("lookup left");
        let right_node = lookup(&fs, ROOT_ID, "right").expect("lookup right");
        let file = lookup(&fs, left_node.inode, "file").expect("lookup file");
        let file_name = CString::new("file").expect("file");
        let moved_name = CString::new("moved").expect("moved");

        let rename_error = match fs.rename(
            ctx(),
            left_node.inode,
            file_name.as_c_str(),
            right_node.inode,
            moved_name.as_c_str(),
            0,
        ) {
            Ok(_) => panic!("cross-mount rename succeeded"),
            Err(error) => error,
        };
        assert_eq!(rename_error.raw_os_error(), Some(libc::EXDEV));

        let link_error = match fs.link(ctx(), file.inode, right_node.inode, moved_name.as_c_str()) {
            Ok(_) => panic!("cross-mount link succeeded"),
            Err(error) => error,
        };
        assert_eq!(link_error.raw_os_error(), Some(libc::EXDEV));
    }

    #[test]
    fn open_handle_survives_unlink_for_reads() {
        let test_dir = TestDir::new("open-unlink");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"still here").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let (handle, _options) = fs
            .open(ctx(), file.inode, false, libc::O_RDONLY as u32)
            .expect("open file");
        let handle = handle.expect("handle");
        let name = CString::new("file").expect("name");
        fs.unlink(ctx(), workspace.inode, name.as_c_str())
            .expect("unlink file");

        let mut reader = VecWriter::default();
        fs.read(ctx(), file.inode, handle, &mut reader, 64, 0, None, 0)
            .expect("read after unlink");
        assert_eq!(reader.data, b"still here");
    }

    #[test]
    fn lookup_forget_counts_saturate_and_reject_malformed_components() {
        let test_dir = TestDir::new("lookup-forget");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

        assert_eq!(
            raw_error(lookup(&fs, workspace.inode, "."), "lookup dot"),
            Some(libc::ENOENT)
        );
        assert_eq!(
            raw_error(
                lookup(&fs, workspace.inode, "nested/path"),
                "lookup slash component"
            ),
            Some(libc::ENOENT)
        );

        let first = lookup(&fs, workspace.inode, "file").expect("first lookup");
        let second = lookup(&fs, workspace.inode, "file").expect("second lookup");
        assert_eq!(first.inode, second.inode);
        assert_eq!(lookup_count(&fs, first.inode), 2);
        let root_lookup_count = lookup_count(&fs, ROOT_ID);

        fs.forget(ctx(), first.inode, 1);
        assert_eq!(lookup_count(&fs, first.inode), 1);
        fs.batch_forget(ctx(), vec![(first.inode, 99), (ROOT_ID, 99)]);
        assert_eq!(lookup_count(&fs, first.inode), 0);
        assert_eq!(lookup_count(&fs, ROOT_ID), root_lookup_count);
    }

    #[test]
    fn release_with_wrong_inode_does_not_consume_valid_handle() {
        let test_dir = TestDir::new("release-wrong-inode");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("left"), b"left-data").expect("write left");
        fs::write(root.join("right"), b"right-data").expect("write right");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let left = lookup(&fs, workspace.inode, "left").expect("lookup left");
        let right = lookup(&fs, workspace.inode, "right").expect("lookup right");
        let (handle, _options) = fs
            .open(ctx(), left.inode, false, libc::O_RDONLY as u32)
            .expect("open left");
        let handle = handle.expect("handle");

        assert_eq!(
            raw_error(
                fs.release(ctx(), right.inode, 0, handle, false, false, None),
                "mismatched release",
            ),
            Some(libc::EBADF)
        );

        let mut reader = VecWriter::default();
        fs.read(ctx(), left.inode, handle, &mut reader, 64, 0, None, 0)
            .expect("handle remains valid after rejected release");
        assert_eq!(reader.data, b"left-data");
        fs.release(ctx(), left.inode, 0, handle, false, false, None)
            .expect("correct release");
        assert_eq!(
            raw_error(
                fs.read(
                    ctx(),
                    left.inode,
                    handle,
                    &mut VecWriter::default(),
                    64,
                    0,
                    None,
                    0
                ),
                "read released handle",
            ),
            Some(libc::EBADF)
        );
    }

    #[test]
    fn open_handle_survives_rename_for_writes() {
        let test_dir = TestDir::new("open-rename");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"hello").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let (handle, _options) = fs
            .open(ctx(), file.inode, false, libc::O_RDWR as u32)
            .expect("open file");
        let handle = handle.expect("handle");
        let old_name = CString::new("file").expect("old");
        let new_name = CString::new("renamed").expect("new");

        fs.rename(
            ctx(),
            workspace.inode,
            old_name.as_c_str(),
            workspace.inode,
            new_name.as_c_str(),
            0,
        )
        .expect("rename while open");
        fs.write(
            ctx(),
            file.inode,
            handle,
            VecReader {
                data: b" after".to_vec(),
            },
            6,
            5,
            None,
            false,
            false,
            0,
        )
        .expect("write through renamed handle");
        fs.release(ctx(), file.inode, 0, handle, true, false, None)
            .expect("release");

        assert!(!root.join("file").exists());
        assert_eq!(
            fs::read(root.join("renamed")).expect("renamed contents"),
            b"hello after"
        );
    }

    #[test]
    fn lookup_after_rename_refreshes_reused_host_inode_path() {
        let test_dir = TestDir::new("rename-refresh");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("file"), b"renamed-data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let old_name = CString::new("file").expect("old");
        let new_name = CString::new("renamed").expect("new");

        fs.rename(
            ctx(),
            workspace.inode,
            old_name.as_c_str(),
            workspace.inode,
            new_name.as_c_str(),
            0,
        )
        .expect("rename");
        let renamed = lookup(&fs, workspace.inode, "renamed").expect("lookup renamed");

        assert_eq!(renamed.inode, file.inode);
        let (handle, _options) = fs
            .open(ctx(), renamed.inode, false, libc::O_RDONLY as u32)
            .expect("open renamed");
        let handle = handle.expect("handle");
        let mut reader = VecWriter::default();
        fs.read(ctx(), renamed.inode, handle, &mut reader, 64, 0, None, 0)
            .expect("read renamed");
        assert_eq!(reader.data, b"renamed-data");
    }

    #[test]
    fn concurrent_distinct_offset_writes_share_handle_safely() {
        let test_dir = TestDir::new("concurrent-writes");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let block = 32usize;
        let writers = 8usize;
        fs::write(root.join("file"), vec![0; block * writers]).expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = std::sync::Arc::new(ComposedFs::new(namespace));
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let file = lookup(&fs, workspace.inode, "file").expect("lookup file");
        let (handle, _options) = fs
            .open(ctx(), file.inode, false, libc::O_RDWR as u32)
            .expect("open file");
        let handle = handle.expect("handle");

        let threads = (0..writers)
            .map(|index| {
                let fs = std::sync::Arc::clone(&fs);
                let inode = file.inode;
                thread::spawn(move || {
                    let data = vec![b'a' + index as u8; block];
                    fs.write(
                        ctx(),
                        inode,
                        handle,
                        VecReader { data },
                        block as u32,
                        (index * block) as u64,
                        None,
                        false,
                        false,
                        0,
                    )
                    .expect("thread write");
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().expect("writer thread");
        }
        fs.fsync(ctx(), file.inode, false, handle).expect("fsync");

        let mut reader = VecWriter::default();
        fs.read(
            ctx(),
            file.inode,
            handle,
            &mut reader,
            (block * writers) as u32,
            0,
            None,
            0,
        )
        .expect("read merged writes");
        for index in 0..writers {
            assert_eq!(
                &reader.data[index * block..(index + 1) * block],
                vec![b'a' + index as u8; block].as_slice()
            );
        }
        fs.release(ctx(), file.inode, 0, handle, true, false, None)
            .expect("release");
    }

    #[test]
    fn readdir_iterator_is_snapshot_when_directory_mutates() {
        let test_dir = TestDir::new("readdir-mutate");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("a"), b"a").expect("write a");
        fs::write(root.join("b"), b"b").expect("write b");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let mut iter = fs
            .readdir(ctx(), workspace.inode, workspace.inode, 4096, 0)
            .expect("readdir snapshot");
        let first = iter.next().expect("first entry").name.to_bytes().to_vec();

        fs::remove_file(root.join("b")).expect("remove b");
        fs::write(root.join("c"), b"c").expect("write c");

        let remaining = dir_names(iter);
        assert_eq!(first, b"a");
        assert_eq!(remaining, vec!["b".to_string()]);

        let fresh = dir_names(
            fs.readdir(ctx(), workspace.inode, workspace.inode, 4096, 0)
                .expect("fresh readdir"),
        );
        assert_eq!(fresh, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn unsupported_device_mknod_fails_without_creating_host_node() {
        let test_dir = TestDir::new("unsupported-mknod");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let name = CString::new("device").expect("device");

        assert_eq!(
            raw_error(
                fs.mknod(
                    ctx(),
                    workspace.inode,
                    name.as_c_str(),
                    libc::S_IFCHR | 0o600,
                    0,
                    0,
                    Extensions::default(),
                ),
                "char device mknod",
            ),
            Some(libc::EPERM)
        );
        assert!(!root.join("device").exists());
    }

    #[test]
    #[ignore = "property stress: run explicitly with `cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_flat_file_operation_sequences -- --ignored --nocapture`"]
    fn proptest_flat_file_operation_sequences() {
        let mut runner = TestRunner::new(Config {
            cases: 128,
            max_shrink_iters: 2048,
            failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
            ..Config::default()
        });
        runner
            .run(&fs_stress_ops(), |ops| {
                run_flat_file_ops_case("proptest-flat-sequence", &ops);
                Ok(())
            })
            .expect("proptest flat file operation sequence");
    }

    #[test]
    #[ignore = "stress regression: run explicitly with `cargo test --manifest-path composed-fs/Cargo.toml --offline stress_seeded_flat_file_operation_sequences -- --ignored --nocapture`"]
    fn stress_seeded_flat_file_operation_sequences() {
        const SEED: u64 = 0x5eed_f17e_2026_0514;
        const STEPS: usize = 512;
        let test_dir = TestDir::new("stress-flat-sequence");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
        let mut rng = TestRng::new(SEED);
        let mut model = BTreeMap::<String, Vec<u8>>::new();
        let mut ops = Vec::<String>::new();

        for step in 0..STEPS {
            let op = if model.is_empty() { 0 } else { rng.usize(6) };
            match op {
                0 => {
                    let name = format!("f{:03}.txt", rng.usize(128));
                    let len = 1 + rng.usize(96);
                    let fill = b'a' + rng.usize(26) as u8;
                    let data = vec![fill; len];
                    ops.push(format!(
                        "{step}: create/truncate {name} len={len} fill={fill}"
                    ));
                    let c_name = CString::new(name.as_str()).expect("name");
                    let (entry, handle, _options) = traced(
                        fs.create(
                            ctx(),
                            workspace.inode,
                            c_name.as_c_str(),
                            0o644,
                            false,
                            (libc::O_RDWR | libc::O_TRUNC) as u32,
                            0,
                            Extensions::default(),
                        ),
                        SEED,
                        &ops,
                        "create",
                    );
                    let handle = handle.expect("handle");
                    traced(
                        fs.write(
                            ctx(),
                            entry.inode,
                            handle,
                            VecReader { data: data.clone() },
                            data.len() as u32,
                            0,
                            None,
                            false,
                            false,
                            0,
                        ),
                        SEED,
                        &ops,
                        "write created file",
                    );
                    traced(
                        fs.release(ctx(), entry.inode, 0, handle, true, false, None),
                        SEED,
                        &ops,
                        "release created file",
                    );
                    model.insert(name, data);
                }
                1 => {
                    let name = model.keys().nth(rng.usize(model.len())).unwrap().clone();
                    let expected = model.get(&name).expect("model entry").clone();
                    ops.push(format!("{step}: lookup/read {name} len={}", expected.len()));
                    let entry = traced(lookup(&fs, workspace.inode, &name), SEED, &ops, "lookup");
                    let (handle, _options) = traced(
                        fs.open(ctx(), entry.inode, false, libc::O_RDONLY as u32),
                        SEED,
                        &ops,
                        "open read",
                    );
                    let handle = handle.expect("handle");
                    let mut reader = VecWriter::default();
                    traced(
                        fs.read(ctx(), entry.inode, handle, &mut reader, 256, 0, None, 0),
                        SEED,
                        &ops,
                        "read",
                    );
                    assert_eq!(
                        reader.data,
                        expected,
                        "seed {SEED:#x} mismatch after trace:\n{}",
                        ops.join("\n")
                    );
                    traced(
                        fs.release(ctx(), entry.inode, 0, handle, false, false, None),
                        SEED,
                        &ops,
                        "release read",
                    );
                    fs.forget(ctx(), entry.inode, 1);
                }
                2 => {
                    let name = model.keys().nth(rng.usize(model.len())).unwrap().clone();
                    let new_name = format!("renamed-{:03}.txt", rng.usize(128));
                    ops.push(format!("{step}: rename {name} -> {new_name}"));
                    let old_c = CString::new(name.as_str()).expect("old");
                    let new_c = CString::new(new_name.as_str()).expect("new");
                    traced(
                        fs.rename(
                            ctx(),
                            workspace.inode,
                            old_c.as_c_str(),
                            workspace.inode,
                            new_c.as_c_str(),
                            0,
                        ),
                        SEED,
                        &ops,
                        "rename",
                    );
                    let data = model.remove(&name).expect("old model");
                    model.insert(new_name, data);
                }
                3 => {
                    let name = model.keys().nth(rng.usize(model.len())).unwrap().clone();
                    ops.push(format!("{step}: unlink {name}"));
                    let c_name = CString::new(name.as_str()).expect("name");
                    traced(
                        fs.unlink(ctx(), workspace.inode, c_name.as_c_str()),
                        SEED,
                        &ops,
                        "unlink",
                    );
                    model.remove(&name);
                }
                4 => {
                    let name = format!("host-{:03}.txt", rng.usize(128));
                    let data = format!("host-step-{step}").into_bytes();
                    ops.push(format!("{step}: host-write {name} len={}", data.len()));
                    fs::write(root.join(&name), &data).unwrap_or_else(|error| {
                        panic!(
                            "host write failed for seed {SEED:#x}: {error}\noperation trace:\n{}",
                            ops.join("\n")
                        )
                    });
                    model.insert(name, data);
                }
                _ => {
                    ops.push(format!("{step}: readdir expect {} files", model.len()));
                    let mut names = dir_names(traced(
                        fs.readdir(ctx(), workspace.inode, workspace.inode, 16 * 1024, 0),
                        SEED,
                        &ops,
                        "readdir",
                    ));
                    names.sort();
                    let expected = model.keys().cloned().collect::<Vec<_>>();
                    assert_eq!(
                        names,
                        expected,
                        "seed {SEED:#x} readdir mismatch after trace:\n{}",
                        ops.join("\n")
                    );
                }
            }
        }

        for (name, expected) in model {
            let actual = fs::read(root.join(&name)).unwrap_or_else(|error| {
                panic!(
                    "final host read failed for {name} seed {SEED:#x}: {error}\noperation trace:\n{}",
                    ops.join("\n")
                )
            });
            assert_eq!(
                actual,
                expected,
                "seed {SEED:#x} final host mismatch for {name}\noperation trace:\n{}",
                ops.join("\n")
            );
        }
    }

    #[test]
    fn readonly_mount_rejects_namespace_mutations() {
        let test_dir = TestDir::new("readonly-mutations");
        let root = test_dir.path.join("root");
        fs::create_dir(&root).expect("create root");
        fs::create_dir(root.join("dir")).expect("create dir");
        fs::write(root.join("file"), b"data").expect("write file");
        let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
            "readonly",
            "/readonly",
            &root,
            AccessMode::Ro,
        )]))
        .expect("build namespace");
        let fs = ComposedFs::new(namespace);
        let readonly = lookup(&fs, ROOT_ID, "readonly").expect("lookup readonly");
        let file = lookup(&fs, readonly.inode, "file").expect("lookup file");
        let file_name = CString::new("file").expect("file");
        let dir_name = CString::new("dir").expect("dir");
        let new_name = CString::new("new").expect("new");

        assert_eq!(
            fs.open(ctx(), file.inode, false, libc::O_WRONLY as u32)
                .expect_err("readonly write open")
                .raw_os_error(),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.mkdir(
                    ctx(),
                    readonly.inode,
                    new_name.as_c_str(),
                    0o755,
                    0,
                    Extensions::default(),
                ),
                "readonly mkdir",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.unlink(ctx(), readonly.inode, file_name.as_c_str()),
                "readonly unlink",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.rmdir(ctx(), readonly.inode, dir_name.as_c_str()),
                "readonly rmdir",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.rename(
                    ctx(),
                    readonly.inode,
                    file_name.as_c_str(),
                    readonly.inode,
                    new_name.as_c_str(),
                    0,
                ),
                "readonly rename",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.link(ctx(), file.inode, readonly.inode, new_name.as_c_str()),
                "readonly link",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.symlink(
                    ctx(),
                    file_name.as_c_str(),
                    readonly.inode,
                    new_name.as_c_str(),
                    Extensions::default(),
                ),
                "readonly symlink",
            ),
            Some(libc::EROFS)
        );
        assert_eq!(
            raw_error(
                fs.mknod(
                    ctx(),
                    readonly.inode,
                    new_name.as_c_str(),
                    libc::S_IFREG | 0o644,
                    0,
                    0,
                    Extensions::default(),
                ),
                "readonly mknod",
            ),
            Some(libc::EROFS)
        );
    }
}
