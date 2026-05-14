#![allow(dead_code)]

use std::fs;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use virtiofsd::filesystem::{Context, Entry, FileSystem, ZeroCopyReader, ZeroCopyWriter, ROOT_ID};
use virtiofsd::oslib::{ReadvFlags, WritevFlags};
use virtiofsd::soft_idmap::{GuestGid, GuestUid};

use crate::{
    AccessMode, ComposedFs, Manifest, MetadataPolicy, MetadataSpec, MountKind, MountSpec,
    Namespace, SourceClass, SyntheticSpec, DEFAULT_TAG, SCHEMA_VERSION,
};

pub(crate) struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub(crate) fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agentvm-composed-fs-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct GuestShareFixture {
    pub(crate) root: TestDir,
    pub(crate) workspace: PathBuf,
    pub(crate) readonly: PathBuf,
    pub(crate) config: PathBuf,
}

impl GuestShareFixture {
    pub(crate) fn new(name: &str) -> Self {
        let root = TestDir::new(name);
        let workspace = root.join("workspace");
        let readonly = root.join("readonly");
        let config = root.join("config");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&readonly).expect("create readonly");
        fs::create_dir_all(&config).expect("create config");
        fs::write(workspace.join("project.txt"), b"workspace").expect("write workspace file");
        fs::write(readonly.join("readonly.txt"), b"readonly").expect("write readonly file");
        fs::write(config.join("composed-binds.json"), b"{}").expect("write config file");
        Self {
            root,
            workspace,
            readonly,
            config,
        }
    }

    pub(crate) fn manifest(&self) -> Manifest {
        manifest_with_mounts(vec![
            dir_mount(
                "workspace",
                "/workspace",
                &self.workspace,
                AccessMode::Rw,
                SourceClass::Workspace,
            ),
            dir_mount(
                "readonly",
                "/readonly",
                &self.readonly,
                AccessMode::Ro,
                SourceClass::UserRo,
            ),
            dir_mount(
                "agentvm-config",
                "/run/agentvm-config",
                &self.config,
                AccessMode::Ro,
                SourceClass::SystemRo,
            ),
        ])
    }

    pub(crate) fn filesystem(&self) -> ComposedFs {
        let namespace = Namespace::from_manifest(&self.manifest()).expect("build namespace");
        ComposedFs::new(namespace)
    }
}

pub(crate) fn ctx() -> Context {
    Context {
        uid: GuestUid::from(0),
        gid: GuestGid::from(0),
        pid: 0,
    }
}

pub(crate) fn manifest_with_mounts(mounts: Vec<MountSpec>) -> Manifest {
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

pub(crate) fn dir_mount(
    id: &str,
    guest_path: &str,
    host_path: &Path,
    access: AccessMode,
    source_class: SourceClass,
) -> MountSpec {
    MountSpec {
        id: id.to_string(),
        guest_path: guest_path.to_string(),
        host_path: host_path.display().to_string(),
        kind: MountKind::Dir,
        access,
        source_class,
        required: true,
        bind: true,
        metadata: MetadataSpec {
            uid_gid: MetadataPolicy::Host,
            permissions: MetadataPolicy::Host,
        },
    }
}

pub(crate) fn file_mount(
    id: &str,
    guest_path: &str,
    host_path: &Path,
    access: AccessMode,
    source_class: SourceClass,
) -> MountSpec {
    MountSpec {
        id: id.to_string(),
        guest_path: guest_path.to_string(),
        host_path: host_path.display().to_string(),
        kind: MountKind::File,
        access,
        source_class,
        required: true,
        bind: true,
        metadata: MetadataSpec {
            uid_gid: MetadataPolicy::Host,
            permissions: MetadataPolicy::Host,
        },
    }
}

pub(crate) fn lookup(fs: &ComposedFs, parent: u64, name: &str) -> io::Result<Entry> {
    let name = std::ffi::CString::new(name).expect("test name");
    fs.lookup(ctx(), parent, name.as_c_str())
}

pub(crate) fn lookup_root(fs: &ComposedFs, name: &str) -> io::Result<Entry> {
    lookup(fs, ROOT_ID, name)
}

pub(crate) struct VecReader {
    pub(crate) data: Vec<u8>,
}

impl ZeroCopyReader for VecReader {
    fn write_to_file_at(
        &mut self,
        file: &fs::File,
        count: usize,
        offset: u64,
        _flags: Option<WritevFlags>,
    ) -> io::Result<usize> {
        let count = count.min(self.data.len());
        file.write_at(&self.data[..count], offset)
    }
}

#[derive(Default)]
pub(crate) struct VecWriter {
    pub(crate) data: Vec<u8>,
}

impl ZeroCopyWriter for VecWriter {
    fn read_from_file_at(
        &mut self,
        file: &fs::File,
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
