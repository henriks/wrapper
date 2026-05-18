use super::*;
use crate::manifest::AccessMode;
use crate::test_support::{
    ctx, dir_mount, lookup, manifest_with_mounts, TestDir, VecReader, VecWriter,
};
use std::os::unix::fs::PermissionsExt;
use virtiofsd::filesystem::ROOT_ID;

const OP_LIMIT: usize = 96;
const LOCK_OP_LIMIT: usize = 160;

const PATHS: [&str; 16] = [
    "a.txt",
    "b.txt",
    "dir",
    "dir/a.txt",
    "dir/b.txt",
    "dir/sub",
    "dir/sub/a.txt",
    "dir/sub/b.txt",
    "empty",
    "empty/file.txt",
    "unicode-ish",
    "long-name-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.txt",
    "rename-src.txt",
    "rename-dst.txt",
    "chmod-target.txt",
    "truncate-target.txt",
];

const FILE_PATHS: [&str; 8] = [
    "a.txt",
    "b.txt",
    "dir/a.txt",
    "dir/b.txt",
    "dir/sub/a.txt",
    "rename-src.txt",
    "chmod-target.txt",
    "truncate-target.txt",
];

pub fn run_fs_operation_bytes(data: &[u8]) {
    if data.is_empty() {
        return;
    }

    let mut input = Input::new(data);
    let test_dir = TestDir::new("fuzz-fs-ops");
    let root = test_dir.join("root");
    let oracle = test_dir.join("oracle");
    let readonly = test_dir.join("readonly");
    let outside = test_dir.join("outside-secret.txt");
    fs::create_dir(&root).expect("create root");
    fs::create_dir(&oracle).expect("create oracle");
    fs::create_dir(&readonly).expect("create readonly");
    fs::write(&outside, b"outside-secret").expect("outside secret");
    fs::write(readonly.join("ro.txt"), b"readonly").expect("readonly file");

    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![
        dir_mount(
            "workspace",
            "/workspace",
            &root,
            AccessMode::Rw,
            SourceClass::Workspace,
        ),
        dir_mount(
            "readonly",
            "/readonly",
            &readonly,
            AccessMode::Ro,
            SourceClass::UserRo,
        ),
    ]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let readonly_entry = lookup(&fs, ROOT_ID, "readonly").expect("lookup readonly");
    let mut trace = Vec::new();

    for step in 0..input.op_count(OP_LIMIT) {
        match input.pick(13) {
            0 => {
                let path = input.path();
                trace.push(format!("{step}: mkdir {path}"));
                same_result(
                    fs_mkdir_path(&fs, workspace.inode, path),
                    host_mkdir_path(&oracle, path),
                    &trace,
                    "mkdir",
                );
            }
            1 => {
                let path = input.path();
                let data = input.bytes(96);
                trace.push(format!("{step}: put {path} len={}", data.len()));
                same_result(
                    fs_create_write_release(&fs, workspace.inode, path, data.clone()),
                    host_put_path(&oracle, path, &data),
                    &trace,
                    "put",
                );
            }
            2 => {
                let path = input.path();
                trace.push(format!("{step}: read {path}"));
                same_result(
                    fs_read_path(&fs, workspace.inode, path),
                    host_read_path(&oracle, path),
                    &trace,
                    "read",
                );
            }
            3 => {
                let from = input.path();
                let to = input.path();
                trace.push(format!("{step}: rename {from} -> {to}"));
                same_result(
                    fs_rename_path(&fs, workspace.inode, from, to),
                    fs::rename(oracle.join(from), oracle.join(to)),
                    &trace,
                    "rename",
                );
            }
            4 => {
                let path = input.path();
                trace.push(format!("{step}: unlink {path}"));
                same_result(
                    fs_unlink_path(&fs, workspace.inode, path),
                    fs::remove_file(oracle.join(path)),
                    &trace,
                    "unlink",
                );
            }
            5 => {
                let path = input.path();
                trace.push(format!("{step}: rmdir {path}"));
                same_result(
                    fs_rmdir_path(&fs, workspace.inode, path),
                    fs::remove_dir(oracle.join(path)),
                    &trace,
                    "rmdir",
                );
            }
            6 => {
                let path = input.path();
                let data = input.bytes(96);
                trace.push(format!("{step}: host-put {path} len={}", data.len()));
                same_result(
                    fs::write(root.join(path), &data),
                    fs::write(oracle.join(path), &data),
                    &trace,
                    "host-put",
                );
            }
            7 => {
                let path = input.path();
                trace.push(format!("{step}: host-mkdir {path}"));
                same_result(
                    fs::create_dir(root.join(path)),
                    fs::create_dir(oracle.join(path)),
                    &trace,
                    "host-mkdir",
                );
            }
            8 => {
                let path = input.path();
                trace.push(format!("{step}: readdir {path}"));
                same_result(
                    fs_readdir_names(&fs, workspace.inode, path),
                    host_readdir_names(&oracle, path),
                    &trace,
                    "readdir",
                );
            }
            9 => {
                let path = input.file_path();
                let len = input.pick(160) as u64;
                trace.push(format!("{step}: truncate {path} len={len}"));
                same_result(
                    fs_truncate_path(&fs, workspace.inode, path, len),
                    host_truncate_path(&oracle, path, len),
                    &trace,
                    "truncate",
                );
            }
            10 => {
                let path = input.file_path();
                let mode = [0o600, 0o644, 0o666, 0o700, 0o755][input.pick(5)] as u32;
                trace.push(format!("{step}: chmod {path} mode={mode:o}"));
                same_result(
                    fs_chmod_path(&fs, workspace.inode, path, mode),
                    host_chmod_path(&oracle, path, mode),
                    &trace,
                    "chmod",
                );
            }
            11 => {
                trace.push(format!("{step}: readonly-create-probe"));
                let name = CString::new("blocked.txt").expect("blocked");
                assert_eq!(
                    raw_error(
                        fs.create(
                            ctx(),
                            readonly_entry.inode,
                            name.as_c_str(),
                            0o644,
                            false,
                            libc::O_RDWR as u32,
                            0,
                            Extensions::default(),
                        ),
                        "readonly create probe",
                    ),
                    Some(libc::EROFS),
                    "operation trace:\n{}",
                    trace.join("\n")
                );
                assert!(
                    !readonly.join("blocked.txt").exists(),
                    "readonly probe created host file\noperation trace:\n{}",
                    trace.join("\n")
                );
            }
            12 => {
                trace.push(format!("{step}: cross-mount-rename-probe"));
                let _ = fs::write(root.join("cross-source.txt"), b"cross");
                let _ = lookup(&fs, workspace.inode, "cross-source.txt");
                let old = CString::new("cross-source.txt").expect("old");
                let new = CString::new("cross-target.txt").expect("new");
                assert_eq!(
                    raw_error(
                        fs.rename(
                            ctx(),
                            workspace.inode,
                            old.as_c_str(),
                            readonly_entry.inode,
                            new.as_c_str(),
                            0,
                        ),
                        "cross mount rename probe",
                    ),
                    Some(libc::EXDEV),
                    "operation trace:\n{}",
                    trace.join("\n")
                );
                assert!(
                    !readonly.join("cross-target.txt").exists(),
                    "cross-mount rename created readonly target\noperation trace:\n{}",
                    trace.join("\n")
                );
            }
            _ => unreachable!(),
        }
    }

    for path in PATHS {
        assert_host_path_equiv(&root, &oracle, path, &trace);
    }
}

pub fn run_lock_operation_bytes(data: &[u8]) {
    if data.is_empty() {
        return;
    }

    let mut input = Input::new(data);
    let test_dir = TestDir::new("fuzz-lock-ops");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write file");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
        SourceClass::Workspace,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let file = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup file");
    let (handle, _options) = fs
        .open(ctx(), file.inode, false, libc::O_RDWR as u32)
        .expect("open file");
    let handle = handle.expect("handle");
    let mut model = Vec::<ModelLock>::new();
    let mut trace = Vec::new();

    for step in 0..input.op_count(LOCK_OP_LIMIT) {
        match input.pick(3) {
            0 => {
                let owner = input.pick(6) as u64;
                let kind = input.lock_kind();
                let start = input.pick(64) as u64;
                let len = 1 + input.pick(16) as u64;
                let end = start + len - 1;
                trace.push(format!(
                    "{step}: set owner={owner} kind={kind:?} {start}..={end}"
                ));
                let mut expected_model = model.clone();
                let expected_ok = model_set_lock(&mut expected_model, owner, kind, start, end);
                let result = fs.setlk(
                    ctx(),
                    file.inode,
                    handle,
                    owner,
                    fuse_byte_lock(lock_kind_to_fuse(kind), start, len),
                    0,
                );
                if expected_ok {
                    result.unwrap_or_else(|error| {
                        panic!(
                            "setlk failed unexpectedly: {error}\noperation trace:\n{}",
                            trace.join("\n")
                        )
                    });
                    model_set_lock(&mut model, owner, kind, start, end);
                } else {
                    let error = result.expect_err("conflicting setlk should fail");
                    assert_lock_conflict(error, "fuzz setlk conflict");
                }
            }
            1 => {
                let owner = input.pick(6) as u64;
                let kind = match input.pick(2) {
                    0 => LockKind::Read,
                    _ => LockKind::Write,
                };
                let start = input.pick(64) as u64;
                let len = 1 + input.pick(16) as u64;
                let end = start + len - 1;
                trace.push(format!(
                    "{step}: get owner={owner} kind={kind:?} {start}..={end}"
                ));
                let has_expected_conflict =
                    model_conflict(&model, owner, kind, start, end).is_some();
                let result = fs
                    .getlk(
                        ctx(),
                        file.inode,
                        handle,
                        owner,
                        fuse_byte_lock(lock_kind_to_fuse(kind), start, len),
                        0,
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "getlk failed unexpectedly: {error}\noperation trace:\n{}",
                            trace.join("\n")
                        )
                    });
                if has_expected_conflict {
                    assert_ne!(
                        result.type_,
                        libc::F_UNLCK as u32,
                        "getlk missed conflict\noperation trace:\n{}",
                        trace.join("\n")
                    );
                    assert!(
                        model_has_conflict_matching_result(
                            &model, owner, kind, start, end, &result
                        ),
                        "getlk conflict {result:?} did not match model\noperation trace:\n{}",
                        trace.join("\n")
                    );
                } else {
                    assert_eq!(
                        result.type_,
                        libc::F_UNLCK as u32,
                        "getlk reported unexpected conflict {result:?}\noperation trace:\n{}",
                        trace.join("\n")
                    );
                }
            }
            2 => {
                let owner = input.pick(6) as u64;
                trace.push(format!("{step}: flush owner={owner}"));
                fs.flush(ctx(), file.inode, handle, owner)
                    .unwrap_or_else(|error| {
                        panic!(
                            "flush failed unexpectedly: {error}\noperation trace:\n{}",
                            trace.join("\n")
                        )
                    });
                model.retain(|lock| lock.owner != owner);
            }
            _ => unreachable!(),
        }
    }

    fs.release(ctx(), file.inode, 0, handle, true, false, None)
        .expect("release fuzz lock handle");
}

struct Input<'a> {
    data: &'a [u8],
    cursor: usize,
}

impl<'a> Input<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }

    fn op_count(&self, limit: usize) -> usize {
        self.data.len().min(limit).max(1)
    }

    fn byte(&mut self) -> u8 {
        if self.data.is_empty() {
            return 0;
        }
        let byte = self.data[self.cursor % self.data.len()];
        self.cursor = self.cursor.wrapping_add(1);
        byte
    }

    fn pick(&mut self, choices: usize) -> usize {
        if choices == 0 {
            return 0;
        }
        usize::from(self.byte()) % choices
    }

    fn path(&mut self) -> &'static str {
        PATHS[self.pick(PATHS.len())]
    }

    fn file_path(&mut self) -> &'static str {
        FILE_PATHS[self.pick(FILE_PATHS.len())]
    }

    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.pick(max_len + 1);
        (0..len).map(|_| self.byte()).collect()
    }

    fn lock_kind(&mut self) -> LockKind {
        match self.pick(3) {
            0 => LockKind::Read,
            1 => LockKind::Write,
            _ => LockKind::Unlock,
        }
    }
}

fn raw_error<T>(result: io::Result<T>, label: &str) -> Option<i32> {
    match result {
        Ok(_) => panic!("{label} succeeded unexpectedly"),
        Err(error) => error.raw_os_error(),
    }
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn lookup_relative_inode(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<u64> {
    let mut inode = root_inode;
    if path.is_empty() {
        return Ok(inode);
    }
    for component in path.split('/') {
        inode = lookup(fs, inode, component)?.inode;
    }
    Ok(inode)
}

fn fs_create_write_release(
    fs: &ComposedFs,
    root_inode: u64,
    path: &str,
    data: Vec<u8>,
) -> io::Result<()> {
    let (parent_path, name) = split_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    let (entry, handle, _options) = fs.create(
        ctx(),
        parent,
        name.as_c_str(),
        0o644,
        false,
        (libc::O_RDWR | libc::O_TRUNC) as u32,
        0,
        Extensions::default(),
    )?;
    let handle = handle.expect("handle");
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
    )?;
    fs.release(ctx(), entry.inode, 0, handle, true, false, None)?;
    Ok(())
}

fn fs_read_path(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<Vec<u8>> {
    let inode = lookup_relative_inode(fs, root_inode, path)?;
    let (handle, _options) = fs.open(ctx(), inode, false, libc::O_RDONLY as u32)?;
    let handle = handle.expect("handle");
    let mut reader = VecWriter::default();
    fs.read(ctx(), inode, handle, &mut reader, 4096, 0, None, 0)?;
    fs.release(ctx(), inode, 0, handle, false, false, None)?;
    Ok(reader.data)
}

fn fs_mkdir_path(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<()> {
    let (parent_path, name) = split_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    fs.mkdir(
        ctx(),
        parent,
        name.as_c_str(),
        0o755,
        0,
        Extensions::default(),
    )?;
    Ok(())
}

fn fs_unlink_path(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<()> {
    let (parent_path, name) = split_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    fs.unlink(ctx(), parent, name.as_c_str())
}

fn fs_rmdir_path(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<()> {
    let (parent_path, name) = split_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    fs.rmdir(ctx(), parent, name.as_c_str())
}

fn fs_rename_path(fs: &ComposedFs, root_inode: u64, from: &str, to: &str) -> io::Result<()> {
    let (from_parent_path, from_name) = split_path(from);
    let (to_parent_path, to_name) = split_path(to);
    let from_parent = lookup_relative_inode(fs, root_inode, from_parent_path)?;
    let to_parent = lookup_relative_inode(fs, root_inode, to_parent_path)?;
    let from_name = CString::new(from_name).expect("from");
    let to_name = CString::new(to_name).expect("to");
    fs.rename(
        ctx(),
        from_parent,
        from_name.as_c_str(),
        to_parent,
        to_name.as_c_str(),
        0,
    )
}

fn fs_truncate_path(fs: &ComposedFs, root_inode: u64, path: &str, len: u64) -> io::Result<()> {
    let inode = lookup_relative_inode(fs, root_inode, path)?;
    let mut attr = fuse::SetattrIn::default();
    attr.size = len;
    fs.setattr(ctx(), inode, attr, None, SetattrValid::SIZE)?;
    Ok(())
}

fn fs_chmod_path(fs: &ComposedFs, root_inode: u64, path: &str, mode: u32) -> io::Result<()> {
    let inode = lookup_relative_inode(fs, root_inode, path)?;
    let mut attr = fuse::SetattrIn::default();
    attr.mode = mode;
    fs.setattr(ctx(), inode, attr, None, SetattrValid::MODE)?;
    Ok(())
}

fn fs_readdir_names(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<Vec<String>> {
    let inode = lookup_relative_inode(fs, root_inode, path)?;
    let mut names = dir_names(fs.readdir(ctx(), inode, inode, 16 * 1024, 0)?);
    names.sort();
    Ok(names)
}

fn host_put_path(root: &Path, path: &str, data: &[u8]) -> io::Result<()> {
    let full = root.join(path);
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    std::io::Write::write_all(&mut options.open(full)?, data)
}

fn host_mkdir_path(root: &Path, path: &str) -> io::Result<()> {
    fs::create_dir(root.join(path))
}

fn host_read_path(root: &Path, path: &str) -> io::Result<Vec<u8>> {
    fs::read(root.join(path))
}

fn host_readdir_names(root: &Path, path: &str) -> io::Result<Vec<String>> {
    let mut names = fs::read_dir(root.join(path))?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

fn host_truncate_path(root: &Path, path: &str, len: u64) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .open(root.join(path))?
        .set_len(len)
}

fn host_chmod_path(root: &Path, path: &str, mode: u32) -> io::Result<()> {
    let path = root.join(path);
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}

fn assert_host_path_equiv(root: &Path, oracle: &Path, path: &str, trace: &[String]) {
    let left_path = root.join(path);
    let right_path = oracle.join(path);
    match (
        fs::symlink_metadata(&left_path),
        fs::symlink_metadata(&right_path),
    ) {
        (Ok(left), Ok(right)) => {
            assert_eq!(
                left.file_type().is_dir(),
                right.file_type().is_dir(),
                "file type mismatch at {path}\noperation trace:\n{}",
                trace.join("\n")
            );
            assert_eq!(
                left.file_type().is_file(),
                right.file_type().is_file(),
                "file type mismatch at {path}\noperation trace:\n{}",
                trace.join("\n")
            );
            if left.file_type().is_file() {
                let left_data = fs::read(&left_path).expect("read root file");
                let right_data = fs::read(&right_path).expect("read oracle file");
                assert_eq!(
                    left_data,
                    right_data,
                    "host file data mismatch at {path}\noperation trace:\n{}",
                    trace.join("\n")
                );
            }
        }
        (Err(left), Err(right)) => assert!(
            equivalent_path_errno(left.raw_os_error(), right.raw_os_error()),
            "host path errno mismatch at {path}: root={left}, oracle={right}\noperation trace:\n{}",
            trace.join("\n")
        ),
        (Ok(_), Err(error)) => panic!(
            "root path {path} exists but oracle failed with {error}\noperation trace:\n{}",
            trace.join("\n")
        ),
        (Err(error), Ok(_)) => panic!(
            "root path {path} failed with {error} but oracle exists\noperation trace:\n{}",
            trace.join("\n")
        ),
    }
}

fn same_result<T: PartialEq + std::fmt::Debug>(
    fs_result: io::Result<T>,
    host_result: io::Result<T>,
    trace: &[String],
    label: &str,
) {
    match (fs_result, host_result) {
        (Ok(fs_value), Ok(host_value)) => assert_eq!(
            fs_value,
            host_value,
            "{label} value mismatch\noperation trace:\n{}",
            trace.join("\n")
        ),
        (Err(fs_error), Err(host_error)) => assert!(
            equivalent_path_errno(fs_error.raw_os_error(), host_error.raw_os_error()),
            "{label} errno mismatch: fs={fs_error}, host={host_error}\noperation trace:\n{}",
            trace.join("\n")
        ),
        (Ok(value), Err(host_error)) => panic!(
            "{label} succeeded in composed-fs with {value:?} but host failed with {host_error}\noperation trace:\n{}",
            trace.join("\n")
        ),
        (Err(fs_error), Ok(value)) => panic!(
            "{label} failed in composed-fs with {fs_error} but host succeeded with {value:?}\noperation trace:\n{}",
            trace.join("\n")
        ),
    }
}

fn equivalent_path_errno(left: Option<i32>, right: Option<i32>) -> bool {
    left == right
        || matches!(
            (left, right),
            (Some(libc::ENOENT), Some(libc::ENOTDIR)) | (Some(libc::ENOTDIR), Some(libc::ENOENT))
        )
}

fn dir_names(mut iter: VecDirIter) -> Vec<String> {
    let mut names = Vec::new();
    while let Some(entry) = iter.next() {
        names.push(entry.name.to_str().expect("utf8").to_string());
    }
    names
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockKind {
    Read,
    Write,
    Unlock,
}

#[derive(Clone, Debug)]
struct ModelLock {
    owner: u64,
    kind: LockKind,
    start: u64,
    end: u64,
}

fn lock_kind_to_fuse(kind: LockKind) -> i32 {
    match kind {
        LockKind::Read => libc::F_RDLCK,
        LockKind::Write => libc::F_WRLCK,
        LockKind::Unlock => libc::F_UNLCK,
    }
}

fn fuse_byte_lock(lock_type: i32, start: u64, len: u64) -> fuse::FileLock {
    fuse::FileLock {
        start,
        end: if len == 0 { u64::MAX } else { start + len - 1 },
        type_: lock_type as u32,
        pid: 0,
    }
}

fn ranges_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start <= right_end && right_start <= left_end
}

fn lock_conflicts(existing: &ModelLock, owner: u64, kind: LockKind, start: u64, end: u64) -> bool {
    existing.owner != owner
        && ranges_overlap(existing.start, existing.end, start, end)
        && (existing.kind == LockKind::Write || kind == LockKind::Write)
}

fn model_conflict(
    locks: &[ModelLock],
    owner: u64,
    kind: LockKind,
    start: u64,
    end: u64,
) -> Option<&ModelLock> {
    locks
        .iter()
        .find(|lock| lock_conflicts(lock, owner, kind, start, end))
}

fn model_has_conflict_matching_result(
    locks: &[ModelLock],
    owner: u64,
    kind: LockKind,
    request_start: u64,
    request_end: u64,
    result: &fuse::FileLock,
) -> bool {
    locks.iter().any(|lock| {
        lock_conflicts(lock, owner, kind, request_start, request_end)
            && lock_kind_to_fuse(lock.kind) as u32 == result.type_
            && ranges_overlap(lock.start, lock.end, result.start, result.end)
            && ranges_overlap(request_start, request_end, result.start, result.end)
    })
}

fn model_unlock_owner_range(locks: &mut Vec<ModelLock>, owner: u64, start: u64, end: u64) {
    let mut updated = Vec::new();
    for lock in locks.drain(..) {
        if lock.owner != owner || !ranges_overlap(lock.start, lock.end, start, end) {
            updated.push(lock);
            continue;
        }
        if lock.start < start {
            updated.push(ModelLock {
                end: start - 1,
                ..lock.clone()
            });
        }
        if end < lock.end {
            updated.push(ModelLock {
                start: end + 1,
                ..lock
            });
        }
    }
    *locks = updated;
}

fn model_set_lock(
    locks: &mut Vec<ModelLock>,
    owner: u64,
    kind: LockKind,
    start: u64,
    end: u64,
) -> bool {
    if kind != LockKind::Unlock && model_conflict(locks, owner, kind, start, end).is_some() {
        return false;
    }
    model_unlock_owner_range(locks, owner, start, end);
    if kind != LockKind::Unlock {
        locks.push(ModelLock {
            owner,
            kind,
            start,
            end,
        });
    }
    true
}

fn assert_lock_conflict(error: io::Error, label: &str) {
    let raw = error.raw_os_error();
    assert!(
        matches!(raw, Some(code) if code == libc::EACCES || code == libc::EAGAIN),
        "{label} returned unexpected error {error:?}"
    );
}
