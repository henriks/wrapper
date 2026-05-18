use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use virtiofsd::filesystem::{Entry, ROOT_ID};
use virtiofsd::fuse;
use virtiofsd::soft_idmap::{GuestGid, GuestUid};

use crate::host_ops::*;
use crate::manifest::{AccessMode, Manifest, MountKind};
use crate::{
    attr_from_metadata, dirent_type_from_mode, invalid_input, normalize_guest_path, now_secs,
    parse_octal_mode, path_components, validate_relative_path, validate_single_path_component,
    ATTR_TTL, DEFAULT_TAG, ENTRY_TTL, SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeKind {
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
        identity: HostIdentity,
    },
    Shadow {
        mount: usize,
        relative_path: PathBuf,
    },
}

#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) inode: u64,
    pub(crate) kind: NodeKind,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) lookup_count: u64,
    pub(crate) cached_attr: Option<fuse::Attr>,
}

#[derive(Debug)]
pub(crate) struct MountRuntime {
    pub(crate) id: String,
    pub(crate) root_path: PathBuf,
    pub(crate) root: File,
    pub(crate) kind: MountKind,
    pub(crate) access: AccessMode,
    pub(crate) filter_suffixes: Vec<String>,
    pub(crate) shadow_root_path: Option<PathBuf>,
    pub(crate) shadow_root: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HostIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

impl HostIdentity {
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }

    pub(crate) fn matches(self, metadata: &fs::Metadata) -> bool {
        self.dev == metadata.dev() && self.ino == metadata.ino()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HostKey {
    mount: usize,
    identity: HostIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ShadowKey {
    mount: usize,
    relative_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Namespace {
    pub(crate) nodes: HashMap<u64, Node>,
    pub(crate) children: HashMap<u64, BTreeMap<String, u64>>,
    pub(crate) path_to_inode: HashMap<String, u64>,
    pub(crate) mounts: Vec<MountRuntime>,
    pub(crate) host_inodes: HashMap<HostKey, u64>,
    pub(crate) shadow_inodes: HashMap<ShadowKey, u64>,
    pub(crate) next_inode: u64,
    pub(crate) synthetic_uid: u32,
    pub(crate) synthetic_gid: u32,
    pub(crate) synthetic_dir_mode: u32,
}

impl Namespace {
    pub(crate) fn from_manifest(manifest: &Manifest) -> io::Result<Self> {
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

        if manifest.shadow_root.is_some() && manifest.filters.is_empty() {
            return Err(invalid_input("shadow_root requires at least one filter"));
        }
        for filter in &manifest.filters {
            filter.validate()?;
            if let Some(mount_id) = &filter.mount_id {
                if !manifest.mounts.iter().any(|mount| &mount.id == mount_id) {
                    return Err(invalid_input(format!(
                        "filter references unknown mount_id: {mount_id}"
                    )));
                }
            }
        }
        if !manifest.filters.is_empty() && manifest.shadow_root.is_none() {
            return Err(invalid_input("filters require shadow_root"));
        }
        let shadow_root_path = manifest.shadow_root.as_ref().map(PathBuf::from);
        if let Some(path) = &shadow_root_path {
            if !path.is_absolute() {
                return Err(invalid_input("shadow_root must be absolute"));
            }
            fs::create_dir_all(path)?;
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
            shadow_inodes: HashMap::new(),
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
            let filter_suffixes = manifest
                .filters
                .iter()
                .filter(|filter| filter.mount_id.as_deref().is_none_or(|id| id == mount.id))
                .flat_map(|filter| filter.suffixes.iter().cloned())
                .collect::<Vec<_>>();
            let shadow_root_path_for_mount = (!filter_suffixes.is_empty()).then(|| {
                shadow_root_path
                    .clone()
                    .expect("filters require shadow_root")
            });
            let shadow_root = shadow_root_path_for_mount
                .as_ref()
                .map(|path| open_mount_root(path, MountKind::Dir))
                .transpose()?;
            let root = open_mount_root(Path::new(&mount.host_path), mount.kind)?;
            ns.mounts.push(MountRuntime {
                id: mount.id.clone(),
                root_path: PathBuf::from(&mount.host_path),
                root,
                kind: mount.kind,
                access: mount.access,
                filter_suffixes,
                shadow_root_path: shadow_root_path_for_mount,
                shadow_root,
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

    pub(crate) fn insert_root(&mut self) {
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

    pub(crate) fn ensure_synthetic_dir(
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
                ..
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

    pub(crate) fn insert_node(&mut self, node: Node) {
        if matches!(
            node.kind,
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } | NodeKind::MountRoot { .. }
        ) {
            self.children.entry(node.inode).or_default();
        }
        self.nodes.insert(node.inode, node);
    }

    pub(crate) fn link_child(&mut self, parent: u64, name: &str, child: u64) -> io::Result<()> {
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

    pub(crate) fn allocate_inode(&mut self) -> u64 {
        let inode = self.next_inode;
        self.next_inode += 1;
        inode
    }

    pub(crate) fn attr_for(&self, node: &Node) -> io::Result<fuse::Attr> {
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

    pub(crate) fn entry_for(&self, node: &Node) -> io::Result<Entry> {
        Ok(Entry {
            inode: node.inode,
            generation: 0,
            attr: self.attr_for(node)?,
            attr_timeout: ATTR_TTL,
            entry_timeout: ENTRY_TTL,
        })
    }

    pub(crate) fn host_metadata_for(&self, node: &Node) -> io::Result<Option<fs::Metadata>> {
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
                identity,
            } => {
                let mount = self
                    .mounts
                    .get(*mount)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                let metadata = stat_beneath(mount.root.as_raw_fd(), relative_path)?;
                validate_host_identity(&metadata, *identity)?;
                Ok(Some(metadata))
            }
            NodeKind::OverlayDir {
                mount,
                relative_path,
            } => {
                let mount = self
                    .mounts
                    .get(*mount)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                Ok(Some(stat_beneath(mount.root.as_raw_fd(), relative_path)?))
            }
            NodeKind::Shadow {
                mount,
                relative_path,
            } => Ok(Some(self.shadow_metadata(*mount, relative_path)?)),
            NodeKind::SyntheticDir => Ok(None),
        }
    }

    pub(crate) fn node_is_dir(&self, node: &Node) -> io::Result<bool> {
        Ok(match &node.kind {
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } => true,
            NodeKind::MountRoot { mount } => matches!(self.mounts[*mount].kind, MountKind::Dir),
            NodeKind::Host { .. } | NodeKind::Shadow { .. } => self
                .host_metadata_for(node)?
                .is_some_and(|metadata| metadata.file_type().is_dir()),
        })
    }

    pub(crate) fn host_location(&self, node: &Node) -> Option<(usize, PathBuf)> {
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
                ..
            } => Some((*mount, relative_path.clone())),
            _ => None,
        }
    }

    pub(crate) fn host_file_location(&self, node: &Node) -> Option<(usize, PathBuf)> {
        match &node.kind {
            NodeKind::MountRoot { mount } => Some((*mount, PathBuf::new())),
            NodeKind::Host {
                mount,
                relative_path,
                ..
            } => Some((*mount, relative_path.clone())),
            _ => None,
        }
    }

    pub(crate) fn host_identity(&self, node: &Node) -> Option<HostIdentity> {
        match &node.kind {
            NodeKind::Host { identity, .. } => Some(*identity),
            _ => None,
        }
    }

    pub(crate) fn host_child_location(
        &self,
        parent: u64,
        name: &str,
    ) -> io::Result<(usize, PathBuf)> {
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

    pub(crate) fn host_mount(&self, mount: usize) -> io::Result<&MountRuntime> {
        self.mounts
            .get(mount)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    pub(crate) fn shadow_root(&self, mount: usize) -> io::Result<&File> {
        self.host_mount(mount)?
            .shadow_root
            .as_ref()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    pub(crate) fn shadow_root_path(&self, mount: usize) -> io::Result<&Path> {
        self.host_mount(mount)?
            .shadow_root_path
            .as_deref()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    pub(crate) fn shadow_relative_path(
        &self,
        mount: usize,
        relative_path: &Path,
    ) -> io::Result<PathBuf> {
        validate_relative_path(relative_path)?;
        Ok(Path::new(&self.host_mount(mount)?.id).join(relative_path))
    }

    pub(crate) fn shadow_metadata(
        &self,
        mount: usize,
        relative_path: &Path,
    ) -> io::Result<fs::Metadata> {
        let shadow_relative = self.shadow_relative_path(mount, relative_path)?;
        stat_beneath(self.shadow_root(mount)?.as_raw_fd(), &shadow_relative)
    }

    pub(crate) fn filtered_path(&self, mount: usize, relative_path: &Path) -> io::Result<bool> {
        validate_relative_path(relative_path)?;
        let Some(name) = relative_path.file_name().and_then(|name| name.to_str()) else {
            return Ok(false);
        };
        Ok(self
            .host_mount(mount)?
            .filter_suffixes
            .iter()
            .any(|suffix| name.ends_with(suffix)))
    }

    pub(crate) fn ensure_shadow_parent(
        &self,
        mount: usize,
        relative_path: &Path,
    ) -> io::Result<()> {
        let shadow_root = self.shadow_root_path(mount)?;
        let shadow_relative = self.shadow_relative_path(mount, relative_path)?;
        if let Some(parent) = shadow_relative.parent() {
            fs::create_dir_all(shadow_root.join(parent))?;
        }
        Ok(())
    }

    pub(crate) fn shadow_file_location(&self, node: &Node) -> Option<(usize, PathBuf)> {
        match &node.kind {
            NodeKind::Shadow {
                mount,
                relative_path,
            } => Some((*mount, relative_path.clone())),
            _ => None,
        }
    }

    pub(crate) fn xattr_location(
        &self,
        inode: u64,
    ) -> io::Result<(usize, PathBuf, Option<HostIdentity>)> {
        let node = self
            .nodes
            .get(&inode)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let Some((mount, relative_path)) = self
            .host_file_location(node)
            .or_else(|| self.host_location(node))
        else {
            return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        };
        Ok((mount, relative_path, self.host_identity(node)))
    }

    pub(crate) fn ensure_mount_writable(&self, mount: usize) -> io::Result<()> {
        match self.host_mount(mount)?.access {
            AccessMode::Rw => Ok(()),
            AccessMode::Ro => Err(io::Error::from_raw_os_error(libc::EROFS)),
        }
    }

    pub(crate) fn lookup_host_child(
        &mut self,
        parent: &Node,
        name: &str,
    ) -> io::Result<Option<(u64, Entry)>> {
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
        if self.filtered_path(mount_index, &relative_path)? {
            let metadata = match self.shadow_metadata(mount_index, &relative_path) {
                Ok(metadata) => metadata,
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
                Err(error) => return Err(error),
            };
            let entry = self.entry_for_shadow_metadata(mount_index, relative_path, &metadata);
            return Ok(Some((entry.inode, entry)));
        }
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

    pub(crate) fn get_or_create_host_node(
        &mut self,
        mount_index: usize,
        relative_path: PathBuf,
        metadata: &fs::Metadata,
    ) -> u64 {
        let identity = HostIdentity::from_metadata(metadata);
        let key = HostKey {
            mount: mount_index,
            identity,
        };
        if let Some(inode) = self.host_inodes.get(&key).copied() {
            if let Some(node) = self.nodes.get_mut(&inode) {
                if let NodeKind::Host {
                    mount,
                    relative_path: known_path,
                    identity: known_identity,
                } = &mut node.kind
                {
                    if *mount == mount_index {
                        *known_path = relative_path;
                        *known_identity = identity;
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
                identity,
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

    pub(crate) fn get_or_create_shadow_node(
        &mut self,
        mount_index: usize,
        relative_path: PathBuf,
        metadata: &fs::Metadata,
    ) -> u64 {
        let key = ShadowKey {
            mount: mount_index,
            relative_path: relative_path.clone(),
        };
        if let Some(inode) = self.shadow_inodes.get(&key).copied() {
            if let Some(node) = self.nodes.get_mut(&inode) {
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
            kind: NodeKind::Shadow {
                mount: mount_index,
                relative_path: relative_path.clone(),
            },
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            lookup_count: 0,
            cached_attr: Some(attr_from_metadata(inode, metadata)),
        });
        self.shadow_inodes.insert(key, inode);
        inode
    }

    pub(crate) fn entry_for_shadow_metadata(
        &mut self,
        mount_index: usize,
        relative_path: PathBuf,
        metadata: &fs::Metadata,
    ) -> Entry {
        let inode = self.get_or_create_shadow_node(mount_index, relative_path, metadata);
        let node = self
            .nodes
            .get_mut(&inode)
            .expect("shadow node must exist after insertion");
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

    pub(crate) fn entry_for_host_metadata(
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

    pub(crate) fn forget_inode(&mut self, inode: u64, count: u64) {
        if inode == ROOT_ID {
            return;
        }
        if let Some(node) = self.nodes.get_mut(&inode) {
            node.lookup_count = node.lookup_count.saturating_sub(count);
        }
        self.prune_cold_inode(inode);
    }

    pub(crate) fn prune_cold_inode(&mut self, inode: u64) {
        let Some(node) = self.nodes.get(&inode) else {
            return;
        };
        if node.lookup_count != 0 {
            return;
        }
        if self
            .children
            .get(&inode)
            .is_some_and(|children| !children.is_empty())
        {
            return;
        }
        let kind = node.kind.clone();
        match kind {
            NodeKind::Host {
                mount, identity, ..
            } => {
                self.nodes.remove(&inode);
                self.children.remove(&inode);
                self.host_inodes.remove(&HostKey { mount, identity });
            }
            NodeKind::Shadow {
                mount,
                relative_path,
            } => {
                self.nodes.remove(&inode);
                self.children.remove(&inode);
                self.shadow_inodes.remove(&ShadowKey {
                    mount,
                    relative_path,
                });
            }
            _ => {}
        }
    }

    pub(crate) fn prune_cold_host_shadow_nodes(&mut self) {
        let inodes = self
            .nodes
            .iter()
            .filter_map(|(inode, node)| {
                if *inode == ROOT_ID
                    || node.lookup_count != 0
                    || self
                        .children
                        .get(inode)
                        .is_some_and(|children| !children.is_empty())
                    || !matches!(node.kind, NodeKind::Host { .. } | NodeKind::Shadow { .. })
                {
                    None
                } else {
                    Some(*inode)
                }
            })
            .collect::<Vec<_>>();
        for inode in inodes {
            self.prune_cold_inode(inode);
        }
    }

    pub(crate) fn dirent_type_for(&self, node: &Node) -> io::Result<u32> {
        Ok(match &node.kind {
            NodeKind::SyntheticDir | NodeKind::OverlayDir { .. } => libc::DT_DIR as u32,
            NodeKind::MountRoot { mount } => match self.mounts[*mount].kind {
                MountKind::Dir => libc::DT_DIR as u32,
                MountKind::File => libc::DT_REG as u32,
            },
            NodeKind::Host { .. } | NodeKind::Shadow { .. } => {
                let metadata = self
                    .host_metadata_for(node)?
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
                dirent_type_from_mode(metadata.mode())
            }
        })
    }
}
