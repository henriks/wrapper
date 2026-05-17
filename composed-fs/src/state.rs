use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct FileHandle {
    pub(crate) inode: u64,
    pub(crate) file: File,
    pub(crate) writable: bool,
    pub(crate) lock_path: PathBuf,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

#[derive(Default)]
pub(crate) struct HandleTable {
    pub(crate) next_handle: u64,
    pub(crate) files: HashMap<u64, FileHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LockKey {
    pub(crate) inode: u64,
    pub(crate) handle: u64,
    pub(crate) owner: u64,
}

#[derive(Default)]
pub(crate) struct LockTable {
    pub(crate) files: HashMap<LockKey, Arc<File>>,
}
