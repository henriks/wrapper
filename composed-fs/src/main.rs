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
    Context, DirEntry, DirectoryIterator, Entry, Extensions, FileSystem, FsOptions, OpenOptions,
    SerializableFileSystem, SetattrValid, ZeroCopyReader, ZeroCopyWriter, ROOT_ID,
};
use virtiofsd::fuse;
use virtiofsd::soft_idmap::{GuestGid, GuestUid, Id};
use virtiofsd::vhost_user::VhostUserFsBackendBuilder;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_TAG: &str = "agentvm";
const DEFAULT_THREAD_POOL_SIZE: usize = 1;
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
        if let Some(metadata) = self.host_metadata_for(node)? {
            return Ok(attr_from_metadata(node.inode, &metadata));
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
        let file = open_beneath_for_io(mount.root.as_raw_fd(), &relative_path, open_flags, 0)?;
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
        let file = open_beneath_for_io(mount.root.as_raw_fd(), &relative_path, flags as i32, 0)?;
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
        access_beneath(mount.root.as_raw_fd(), &relative_path, mask as i32)
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
        let file = handles
            .files
            .remove(&handle)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        if file.inode != inode {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
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

fn main() -> io::Result<()> {
    let args = Args::parse()?;
    let manifest_text = fs::read_to_string(&args.manifest)?;
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .map_err(|error| invalid_input(format!("invalid manifest JSON: {error}")))?;
    let namespace = Namespace::from_manifest(&manifest)?;
    let fs = ComposedFs::new(namespace);

    if let Some(parent) = args.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let backend = Arc::new(
        VhostUserFsBackendBuilder::default()
            .set_thread_pool_size(args.thread_pool_size)
            .set_tag(Some(args.tag.clone()))
            .build(fs)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?,
    );
    let listener = Listener::new(&args.socket_path, true)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let mut daemon = VhostUserDaemon::new(
        String::from("agentvm-composed-fs"),
        backend,
        GuestMemoryAtomic::new(GuestMemoryMmap::new()),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;

    eprintln!(
        "agentvm-composed-fs: serving tag {:?} on {}",
        args.tag,
        args.socket_path.display()
    );
    daemon
        .start(listener)
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

fn fstatvfs_file(file: &File) -> io::Result<libc::statvfs64> {
    let mut st: libc::statvfs64 = unsafe { mem::zeroed() };
    let result = unsafe { libc::fstatvfs64(file.as_raw_fd(), &mut st) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
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
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt;
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

    fn lookup(fs: &ComposedFs, parent: u64, name: &str) -> io::Result<Entry> {
        let name = CString::new(name).expect("test name");
        fs.lookup(ctx(), parent, name.as_c_str())
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
}
