use super::*;
use crate::test_support::{ctx, lookup, TestDir, VecReader, VecWriter};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, TestRunner};
use std::os::fd::RawFd;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

fn manifest_with_mounts(mounts: Vec<MountSpec>) -> Manifest {
    test_support::manifest_with_mounts(mounts)
}

fn dir_mount(id: &str, guest_path: &str, host_path: &Path, access: AccessMode) -> MountSpec {
    test_support::dir_mount(id, guest_path, host_path, access, SourceClass::Workspace)
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

fn raw_error<T>(result: io::Result<T>, label: &str) -> Option<i32> {
    match result {
        Ok(_) => panic!("{label} succeeded unexpectedly"),
        Err(error) => error.raw_os_error(),
    }
}

fn assert_poison_error(error: io::Error, expected_lock: &str) {
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(
        error.to_string().contains(expected_lock),
        "expected poison error for {expected_lock}, got {error}"
    );
}

fn poison_for_test(action: impl FnOnce()) {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    std::panic::set_hook(previous_hook);
    assert!(result.is_err(), "poisoning action should panic");
}

fn byte_lock(lock_type: i32, start: libc::off_t, len: libc::off_t) -> libc::flock {
    libc::flock {
        l_type: lock_type as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: start,
        l_len: len,
        l_pid: 0,
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

fn fcntl_lock(fd: RawFd, command: i32, mut lock: libc::flock) -> io::Result<libc::flock> {
    // SAFETY: fcntl is called with a valid fd and a pointer to an initialized flock.
    let result = unsafe { libc::fcntl(fd, command, &mut lock) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(lock)
    }
}

fn fcntl_lock_eventually(fd: RawFd, command: i32, lock: libc::flock) -> io::Result<libc::flock> {
    let mut last_error = None;
    for _ in 0..50 {
        match fcntl_lock(fd, command, lock) {
            Ok(lock) => return Ok(lock),
            Err(error) if matches!(error.raw_os_error(), Some(code) if code == libc::EAGAIN || code == libc::EACCES) =>
            {
                last_error = Some(error);
                thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("lock did not complete")))
}

fn fs_setlk_eventually(
    fs: &ComposedFs,
    inode: u64,
    handle: u64,
    owner: u64,
    lock: fuse::FileLock,
) -> io::Result<()> {
    let mut last_error = None;
    for _ in 0..50 {
        match fs.setlk(ctx(), inode, handle, owner, lock, 0) {
            Ok(()) => return Ok(()),
            Err(error) if matches!(error.raw_os_error(), Some(code) if code == libc::EAGAIN || code == libc::EACCES) =>
            {
                last_error = Some(error);
                thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("fs lock did not complete")))
}

fn assert_lock_conflict(error: io::Error, label: &str) {
    let raw = error.raw_os_error();
    assert!(
        matches!(raw, Some(code) if code == libc::EACCES || code == libc::EAGAIN),
        "{label} returned unexpected error {error:?}"
    );
}

fn single_mount_filesystem(name: &str) -> (TestDir, PathBuf, ComposedFs) {
    let test_dir = TestDir::new(name);
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("file.txt"), b"contents").expect("write file");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let filesystem = ComposedFs::new(namespace);
    (test_dir, root, filesystem)
}

#[test]
fn poisoned_state_locks_return_io_errors_instead_of_panicking() {
    let (_test_dir, _root, filesystem) = single_mount_filesystem("poison-namespace");
    poison_for_test(|| {
        let _guard = filesystem.namespace.write().expect("namespace lock");
        panic!("poison namespace lock");
    });
    let error = filesystem.getattr(ctx(), ROOT_ID, None).unwrap_err();
    assert_poison_error(error, "namespace");

    let (_test_dir, root, filesystem) = single_mount_filesystem("poison-handles");
    poison_for_test(|| {
        let _guard = filesystem.handles.write().expect("handle table lock");
        panic!("poison handle table lock");
    });
    let file = File::open(root.join("file.txt")).expect("open host file");
    let error = filesystem
        .insert_file_handle(ROOT_ID, file, false, root.join("file.txt"))
        .unwrap_err();
    assert_poison_error(error, "handle table");

    let (_test_dir, _root, filesystem) = single_mount_filesystem("poison-locks");
    poison_for_test(|| {
        let _guard = filesystem.locks.lock().expect("lock table lock");
        panic!("poison lock table lock");
    });
    let error = filesystem
        .remove_released_file_locks(0, 0, None, false)
        .unwrap_err();
    assert_poison_error(error, "lock table");
}

fn child_posix_write_lock_conflicts(path: &Path) -> bool {
    let path = CString::new(path.as_os_str().as_bytes()).expect("cstring path");
    // SAFETY: fork is followed in the child only by async-signal-safe libc calls and _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", io::Error::last_os_error());
    if pid == 0 {
        // SAFETY: path is a valid nul-terminated string and the result is checked.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            unsafe { libc::_exit(10) };
        }
        let mut lock = byte_lock(libc::F_WRLCK, 0, 1);
        // SAFETY: fd is valid and lock points to initialized memory.
        let rc = unsafe { libc::fcntl(fd, libc::F_SETLK, &mut lock) };
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // SAFETY: fd was opened above and the child exits immediately afterward.
        unsafe {
            libc::close(fd);
            if rc == 0 {
                libc::_exit(20);
            }
            if errno == libc::EACCES || errno == libc::EAGAIN {
                libc::_exit(0);
            }
            libc::_exit(30);
        }
    }

    let mut status = 0;
    // SAFETY: pid is the child returned by fork and status points to initialized storage.
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(
        waited,
        pid,
        "waitpid failed: {}",
        io::Error::last_os_error()
    );
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

struct ChildPosixLock {
    pid: libc::pid_t,
    release_fd: RawFd,
}

impl Drop for ChildPosixLock {
    fn drop(&mut self) {
        let byte = [1_u8];
        // SAFETY: release_fd is owned by this guard and points to the child release pipe.
        unsafe {
            libc::write(self.release_fd, byte.as_ptr().cast(), byte.len());
            libc::close(self.release_fd);
        }
        let mut status = 0;
        // SAFETY: pid is the child returned by fork and status points to initialized storage.
        let waited = unsafe { libc::waitpid(self.pid, &mut status, 0) };
        assert_eq!(
            waited,
            self.pid,
            "waitpid failed: {}",
            io::Error::last_os_error()
        );
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "child lock holder exited with status {status}"
        );
    }
}

fn child_hold_posix_write_lock(
    path: &Path,
    start: libc::off_t,
    len: libc::off_t,
) -> ChildPosixLock {
    let path = CString::new(path.as_os_str().as_bytes()).expect("cstring path");
    let mut ready = [0; 2];
    let mut release = [0; 2];
    // SAFETY: pipe initializes both fd arrays on success.
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0, "ready pipe");
    assert_eq!(
        unsafe { libc::pipe(release.as_mut_ptr()) },
        0,
        "release pipe"
    );
    // SAFETY: fork is followed in the child only by simple libc calls and _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", io::Error::last_os_error());
    if pid == 0 {
        unsafe {
            libc::close(ready[0]);
            libc::close(release[1]);
        }
        // SAFETY: path is a valid nul-terminated string and the result is checked.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            unsafe { libc::_exit(10) };
        }
        let mut lock = byte_lock(libc::F_WRLCK, start, len);
        // SAFETY: fd is valid and lock points to initialized memory.
        if unsafe { libc::fcntl(fd, libc::F_SETLK, &mut lock) } < 0 {
            unsafe { libc::_exit(20) };
        }
        let byte = [1_u8];
        unsafe {
            libc::write(ready[1], byte.as_ptr().cast(), byte.len());
            libc::close(ready[1]);
        }
        let mut buf = [0_u8; 1];
        unsafe {
            libc::read(release[0], buf.as_mut_ptr().cast(), buf.len());
            libc::close(release[0]);
            libc::close(fd);
            libc::_exit(0);
        }
    }

    unsafe {
        libc::close(ready[1]);
        libc::close(release[0]);
    }
    let mut buf = [0_u8; 1];
    // SAFETY: ready[0] is the parent read end of the child readiness pipe.
    let read = unsafe { libc::read(ready[0], buf.as_mut_ptr().cast(), buf.len()) };
    unsafe { libc::close(ready[0]) };
    assert_eq!(read, 1, "child did not report lock readiness");
    ChildPosixLock {
        pid,
        release_fd: release[1],
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

fn namespace_cache_counts(fs: &ComposedFs) -> (usize, usize, usize) {
    let namespace = fs.namespace.read().expect("namespace lock");
    (
        namespace.nodes.len(),
        namespace.host_inodes.len(),
        namespace.shadow_inodes.len(),
    )
}

fn namespace_contains_inode(fs: &ComposedFs, inode: u64) -> bool {
    fs.namespace
        .read()
        .expect("namespace lock")
        .nodes
        .contains_key(&inode)
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

#[derive(Clone, Debug)]
enum NestedFsOp {
    Mkdir { path: usize },
    Put { path: usize, len: usize, byte: u8 },
    Read { path: usize },
    Rename { from: usize, to: usize },
    Unlink { path: usize },
    Rmdir { path: usize },
    HostPut { path: usize, len: usize, byte: u8 },
    HostMkdir { path: usize },
    Readdir { path: usize },
    Truncate { path: usize, len: u64 },
    Chmod { path: usize, mode: u32 },
    ReadonlyCreateProbe,
    CrossMountRenameProbe,
    HostSymlinkEscapeProbe,
}

fn nested_fs_ops() -> impl Strategy<Value = Vec<NestedFsOp>> {
    prop::collection::vec(
        prop_oneof![
            (0usize..16).prop_map(|path| NestedFsOp::Mkdir { path }),
            (0usize..16, 0usize..96, any::<u8>()).prop_map(|(path, len, byte)| {
                NestedFsOp::Put {
                    path,
                    len: len + 1,
                    byte,
                }
            }),
            (0usize..16).prop_map(|path| NestedFsOp::Read { path }),
            (0usize..16, 0usize..16).prop_map(|(from, to)| NestedFsOp::Rename { from, to }),
            (0usize..16).prop_map(|path| NestedFsOp::Unlink { path }),
            (0usize..16).prop_map(|path| NestedFsOp::Rmdir { path }),
            (0usize..16, 0usize..96, any::<u8>()).prop_map(|(path, len, byte)| {
                NestedFsOp::HostPut {
                    path,
                    len: len + 1,
                    byte,
                }
            }),
            (0usize..16).prop_map(|path| NestedFsOp::HostMkdir { path }),
            (0usize..16).prop_map(|path| NestedFsOp::Readdir { path }),
            (0usize..16, 0u64..128).prop_map(|(path, len)| NestedFsOp::Truncate { path, len }),
            (
                prop_oneof![
                    Just(0usize),
                    Just(1),
                    Just(3),
                    Just(5),
                    Just(7),
                    Just(8),
                    Just(9),
                    Just(10),
                    Just(11),
                    Just(12),
                    Just(13),
                    Just(14),
                    Just(15),
                ],
                prop_oneof![Just(0o600_u32), Just(0o644), Just(0o700), Just(0o755)]
            )
                .prop_map(|(path, mode)| NestedFsOp::Chmod { path, mode }),
            Just(NestedFsOp::ReadonlyCreateProbe),
            Just(NestedFsOp::CrossMountRenameProbe),
            Just(NestedFsOp::HostSymlinkEscapeProbe),
        ],
        1..96,
    )
}

#[derive(Clone, Debug)]
enum LockStressOp {
    Set {
        handle_slot: usize,
        owner: u64,
        kind: LockKind,
        start: u64,
        len: u64,
    },
    Get {
        handle_slot: usize,
        owner: u64,
        kind: LockKind,
        start: u64,
        len: u64,
    },
    Flush {
        handle_slot: usize,
        owner: u64,
    },
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

fn lock_stress_ops(max_len: usize) -> impl Strategy<Value = Vec<LockStressOp>> {
    prop::collection::vec(
        prop_oneof![
            (0usize..2, 0u64..4, lock_kind_strategy(), 0u64..24, 1u64..8).prop_map(
                |(handle_slot, owner, kind, start, len)| LockStressOp::Set {
                    handle_slot,
                    owner,
                    kind,
                    start,
                    len,
                }
            ),
            (
                0usize..2,
                0u64..4,
                prop_oneof![Just(LockKind::Read), Just(LockKind::Write)],
                0u64..24,
                1u64..8
            )
                .prop_map(|(handle_slot, owner, kind, start, len)| {
                    LockStressOp::Get {
                        handle_slot,
                        owner,
                        kind,
                        start,
                        len,
                    }
                }),
            (0usize..2, 0u64..4)
                .prop_map(|(handle_slot, owner)| LockStressOp::Flush { handle_slot, owner }),
        ],
        1..max_len,
    )
}

fn lock_kind_strategy() -> impl Strategy<Value = LockKind> {
    prop_oneof![
        Just(LockKind::Read),
        Just(LockKind::Write),
        Just(LockKind::Unlock),
    ]
}

fn lock_kind_to_fuse(kind: LockKind) -> i32 {
    match kind {
        LockKind::Read => libc::F_RDLCK,
        LockKind::Write => libc::F_WRLCK,
        LockKind::Unlock => libc::F_UNLCK,
    }
}

fn lock_conflicts(existing: &ModelLock, owner: u64, kind: LockKind, start: u64, end: u64) -> bool {
    existing.owner != owner
        && ranges_overlap(existing.start, existing.end, start, end)
        && (existing.kind == LockKind::Write || kind == LockKind::Write)
}

fn ranges_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start <= right_end && right_start <= left_end
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

fn run_lock_ops_case(case_name: &str, generated_ops: &[LockStressOp]) {
    let test_dir = TestDir::new(case_name);
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write file");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let file = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup file");
    let (first_handle, _options) = fs
        .open(ctx(), file.inode, false, libc::O_RDWR as u32)
        .expect("open file first");
    let first_handle = first_handle.expect("first handle");
    let (second_handle, _options) = fs
        .open(ctx(), file.inode, false, libc::O_RDWR as u32)
        .expect("open file second");
    let second_handle = second_handle.expect("second handle");
    let handles = [first_handle, second_handle];
    let mut model = Vec::new();
    let mut trace = Vec::new();

    for (step, op) in generated_ops.iter().enumerate() {
        match *op {
            LockStressOp::Set {
                handle_slot,
                owner,
                kind,
                start,
                len,
            } => {
                let handle = handles[handle_slot % handles.len()];
                let end = start + len - 1;
                trace.push(format!(
                    "{step}: set handle={handle_slot} owner={owner} kind={kind:?} {start}..={end}"
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
                    assert_lock_conflict(error, "proptest setlk conflict");
                }
            }
            LockStressOp::Get {
                handle_slot,
                owner,
                kind,
                start,
                len,
            } => {
                let handle = handles[handle_slot % handles.len()];
                let end = start + len - 1;
                trace.push(format!(
                    "{step}: get handle={handle_slot} owner={owner} kind={kind:?} {start}..={end}"
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
                                    &model,
                                    owner,
                                    kind,
                                    start,
                                    end,
                                    &result
                                ),
                                "getlk conflict {result:?} did not match any model conflict\noperation trace:\n{}",
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
            LockStressOp::Flush { handle_slot, owner } => {
                let handle = handles[handle_slot % handles.len()];
                trace.push(format!("{step}: flush handle={handle_slot} owner={owner}"));
                fs.flush(ctx(), file.inode, handle, owner)
                    .unwrap_or_else(|error| {
                        panic!(
                            "flush failed unexpectedly: {error}\noperation trace:\n{}",
                            trace.join("\n")
                        )
                    });
                model.retain(|lock| lock.owner != owner);
            }
        }
    }
}

fn nested_path(index: usize) -> &'static str {
    match index % 16 {
        0 => "a.txt",
        1 => "b.txt",
        2 => "dir",
        3 => "dir/a.txt",
        4 => "dir/sub",
        5 => "dir/sub/c.txt",
        6 => "other",
        7 => "other/d.txt",
        8 => ".hidden",
        9 => "UPPER.case",
        10 => "space name.txt",
        11 => "dash_under-123.txt",
        12 => "dir/space child.txt",
        13 => "dir/sub/deep.txt",
        14 => "long-name-abcdefghijklmnopqrstuvwxyz-ABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789.txt",
        _ => "dir/long-child-abcdefghijklmnopqrstuvwxyz-ABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789.txt",
    }
}

fn split_nested_path(path: &str) -> (&str, &str) {
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
    let (parent_path, name) = split_nested_path(path);
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
    let (parent_path, name) = split_nested_path(path);
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
    let (parent_path, name) = split_nested_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    fs.unlink(ctx(), parent, name.as_c_str())
}

fn fs_rmdir_path(fs: &ComposedFs, root_inode: u64, path: &str) -> io::Result<()> {
    let (parent_path, name) = split_nested_path(path);
    let parent = lookup_relative_inode(fs, root_inode, parent_path)?;
    let name = CString::new(name).expect("name");
    fs.rmdir(ctx(), parent, name.as_c_str())
}

fn fs_rename_path(fs: &ComposedFs, root_inode: u64, from: &str, to: &str) -> io::Result<()> {
    let (from_parent_path, from_name) = split_nested_path(from);
    let (to_parent_path, to_name) = split_nested_path(to);
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

fn run_nested_fs_ops_case(case_name: &str, generated_ops: &[NestedFsOp]) {
    let test_dir = TestDir::new(case_name);
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
        dir_mount("workspace", "/workspace", &root, AccessMode::Rw),
        dir_mount("readonly", "/readonly", &readonly, AccessMode::Ro),
    ]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let readonly_entry = lookup(&fs, ROOT_ID, "readonly").expect("lookup readonly");
    let mut trace = Vec::new();

    for (step, op) in generated_ops.iter().enumerate() {
        match op {
            NestedFsOp::Mkdir { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: mkdir {path}"));
                same_result(
                    fs_mkdir_path(&fs, workspace.inode, path),
                    host_mkdir_path(&oracle, path),
                    &trace,
                    "mkdir",
                );
            }
            NestedFsOp::Put { path, len, byte } => {
                let path = nested_path(*path);
                let data = vec![*byte; *len];
                trace.push(format!("{step}: put {path} len={len} byte={byte}"));
                same_result(
                    fs_create_write_release(&fs, workspace.inode, path, data.clone()),
                    host_put_path(&oracle, path, &data),
                    &trace,
                    "put",
                );
            }
            NestedFsOp::Read { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: read {path}"));
                same_result(
                    fs_read_path(&fs, workspace.inode, path),
                    host_read_path(&oracle, path),
                    &trace,
                    "read",
                );
            }
            NestedFsOp::Rename { from, to } => {
                let from = nested_path(*from);
                let to = nested_path(*to);
                trace.push(format!("{step}: rename {from} -> {to}"));
                same_result(
                    fs_rename_path(&fs, workspace.inode, from, to),
                    fs::rename(oracle.join(from), oracle.join(to)),
                    &trace,
                    "rename",
                );
            }
            NestedFsOp::Unlink { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: unlink {path}"));
                same_result(
                    fs_unlink_path(&fs, workspace.inode, path),
                    fs::remove_file(oracle.join(path)),
                    &trace,
                    "unlink",
                );
            }
            NestedFsOp::Rmdir { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: rmdir {path}"));
                same_result(
                    fs_rmdir_path(&fs, workspace.inode, path),
                    fs::remove_dir(oracle.join(path)),
                    &trace,
                    "rmdir",
                );
            }
            NestedFsOp::HostPut { path, len, byte } => {
                let path = nested_path(*path);
                let data = vec![*byte; *len];
                trace.push(format!("{step}: host-put {path} len={len} byte={byte}"));
                same_result(
                    fs::write(root.join(path), &data),
                    fs::write(oracle.join(path), &data),
                    &trace,
                    "host-put",
                );
            }
            NestedFsOp::HostMkdir { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: host-mkdir {path}"));
                same_result(
                    fs::create_dir(root.join(path)),
                    fs::create_dir(oracle.join(path)),
                    &trace,
                    "host-mkdir",
                );
            }
            NestedFsOp::Readdir { path } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: readdir {path}"));
                same_result(
                    fs_readdir_names(&fs, workspace.inode, path),
                    host_readdir_names(&oracle, path),
                    &trace,
                    "readdir",
                );
            }
            NestedFsOp::Truncate { path, len } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: truncate {path} len={len}"));
                same_result(
                    fs_truncate_path(&fs, workspace.inode, path, *len),
                    host_truncate_path(&oracle, path, *len),
                    &trace,
                    "truncate",
                );
            }
            NestedFsOp::Chmod { path, mode } => {
                let path = nested_path(*path);
                trace.push(format!("{step}: chmod {path} mode={mode:o}"));
                same_result(
                    fs_chmod_path(&fs, workspace.inode, path, *mode),
                    host_chmod_path(&oracle, path, *mode),
                    &trace,
                    "chmod",
                );
            }
            NestedFsOp::ReadonlyCreateProbe => {
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
            NestedFsOp::CrossMountRenameProbe => {
                trace.push(format!("{step}: cross-mount-rename-probe"));
                fs::write(root.join("cross-source.txt"), b"cross").expect("cross source");
                lookup(&fs, workspace.inode, "cross-source.txt").expect("source lookup");
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
            NestedFsOp::HostSymlinkEscapeProbe => {
                trace.push(format!("{step}: host-symlink-escape-probe"));
                let link = root.join("escape-link");
                let _ = fs::remove_file(&link);
                symlink(&outside, &link).expect("create escape symlink");
                match fs_read_path(&fs, workspace.inode, "escape-link") {
                    Ok(data) => assert_ne!(
                        data,
                        b"outside-secret",
                        "symlink escape exposed outside file\noperation trace:\n{}",
                        trace.join("\n")
                    ),
                    Err(error) => assert!(
                        matches!(
                            error.raw_os_error(),
                            Some(libc::ENOENT | libc::ELOOP | libc::EXDEV | libc::ENOTDIR)
                        ),
                        "unexpected symlink escape error {error}\noperation trace:\n{}",
                        trace.join("\n")
                    ),
                }
            }
        }
    }
}

fn run_flat_file_ops_case(case_name: &str, generated_ops: &[FsStressOp]) {
    let test_dir = TestDir::new(case_name);
    let root = test_dir.join("root");
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
fn manifest_shape_validation_does_not_open_host_sources() {
    let manifest = r#"{
            "schema_version": 1,
            "mounts": [{
                "id": "missing-but-shape-valid",
                "guest_path": "/workspace",
                "host_path": "/definitely/missing/agentvm/fuzz/source",
                "kind": "dir",
                "access": "rw",
                "source_class": "workspace",
                "required": true,
                "bind": true,
                "metadata": { "uid_gid": "host", "permissions": "host" }
            }]
        }"#;

    validate_manifest_json_shape(manifest).expect("shape validation should not open host path");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
    let gh = test_dir.join("gh");
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
    let root = test_dir.join("root");
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
fn init_advertises_posix_locks_when_guest_offers_them() {
    let test_dir = TestDir::new("lock-policy");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);

    let options = fs
        .init(FsOptions::BIG_WRITES | FsOptions::POSIX_LOCKS)
        .expect("init");
    assert!(options.contains(FsOptions::BIG_WRITES));
    assert!(options.contains(FsOptions::POSIX_LOCKS));
}

#[test]
fn ofd_locks_conflict_with_host_posix_locks_across_processes() {
    let test_dir = TestDir::new("ofd-posix-conflict");
    let path = test_dir.join("db.sqlite");
    fs::write(&path, b"sqlite-lock-probe").expect("write probe");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open probe");

    fcntl_lock(
        file.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_WRLCK, 0, 1),
    )
    .expect("set ofd lock");
    assert!(
        child_posix_write_lock_conflicts(&path),
        "host POSIX lock unexpectedly ignored existing OFD lock"
    );
    fcntl_lock(
        file.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_UNLCK, 0, 1),
    )
    .expect("unlock ofd lock");
}

#[test]
fn separate_ofd_descriptions_model_distinct_guest_lock_owners() {
    let test_dir = TestDir::new("ofd-owner-model");
    let path = test_dir.join("state.sqlite");
    fs::write(&path, b"sqlite-lock-probe").expect("write probe");
    let owner_a = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open owner a");
    let owner_b = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open owner b");

    fcntl_lock(
        owner_a.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_WRLCK, 0, 8),
    )
    .expect("set owner a lock");
    let conflict = fcntl_lock(
        owner_b.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_WRLCK, 0, 8),
    )
    .expect_err("owner b should conflict");
    assert_lock_conflict(conflict, "owner b OFD lock");

    // Closing a duplicate of owner A must not release the lock while owner A remains open.
    // SAFETY: dup is called on a valid fd and the result is checked below.
    let dup = unsafe { libc::dup(owner_a.as_raw_fd()) };
    assert!(dup >= 0, "dup failed: {}", io::Error::last_os_error());
    // SAFETY: dup returned a new fd owned by this test.
    assert_eq!(unsafe { libc::close(dup) }, 0, "close dup failed");
    let conflict = fcntl_lock(
        owner_b.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_WRLCK, 0, 8),
    )
    .expect_err("closing a dup should not release owner a lock");
    assert_lock_conflict(conflict, "owner b after dup close");

    fcntl_lock(
        owner_a.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_UNLCK, 0, 8),
    )
    .expect("unlock owner a");
    fcntl_lock(
        owner_b.as_raw_fd(),
        libc::F_OFD_SETLK,
        byte_lock(libc::F_WRLCK, 0, 8),
    )
    .expect("owner b lock after owner a unlock");
}

#[test]
fn composed_lock_bridge_conflicts_guest_owners_and_host_posix_locks() {
    let test_dir = TestDir::new("lock-bridge-conflict");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");

    let mut guest_pid_lock = fuse_byte_lock(libc::F_WRLCK, 0, 1);
    guest_pid_lock.pid = 1234;
    fs.setlk(ctx(), db.inode, handle, 10, guest_pid_lock, 0)
        .expect("owner 10 write lock");
    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            handle,
            20,
            fuse_byte_lock(libc::F_WRLCK, 0, 1),
            0,
        )
        .expect_err("owner 20 should conflict");
    assert_lock_conflict(conflict, "guest owner 20");

    let getlk = fs
        .getlk(
            ctx(),
            db.inode,
            handle,
            20,
            fuse_byte_lock(libc::F_WRLCK, 0, 1),
            0,
        )
        .expect("getlk");
    assert_eq!(getlk.type_, libc::F_WRLCK as u32);
    assert_eq!(getlk.start, 0);
    assert_eq!(getlk.end, 0);
    assert!(
        child_posix_write_lock_conflicts(&root.join("state.sqlite")),
        "host POSIX lock unexpectedly ignored composed-fs OFD lock"
    );

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        10,
        fuse_byte_lock(libc::F_UNLCK, 0, 1),
        0,
    )
    .expect("owner 10 unlock");
    fs.setlk(
        ctx(),
        db.inode,
        handle,
        20,
        fuse_byte_lock(libc::F_WRLCK, 0, 1),
        0,
    )
    .expect("owner 20 lock after unlock");
    fs.release(ctx(), db.inode, 0, handle, false, false, None)
        .expect("release");
}

#[test]
fn composed_lock_bridge_shares_guest_owner_across_handles() {
    let test_dir = TestDir::new("lock-bridge-owner-handles");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (first_handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db first");
    let first_handle = first_handle.expect("first handle");
    let (second_handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db second");
    let second_handle = second_handle.expect("second handle");

    fs.setlk(
        ctx(),
        db.inode,
        first_handle,
        10,
        fuse_byte_lock(libc::F_WRLCK, 0, 8),
        0,
    )
    .expect("owner 10 lock through first handle");
    fs.setlk(
        ctx(),
        db.inode,
        second_handle,
        10,
        fuse_byte_lock(libc::F_WRLCK, 0, 8),
        0,
    )
    .expect("same owner lock through second handle reuses owner file");

    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            second_handle,
            20,
            fuse_byte_lock(libc::F_WRLCK, 0, 8),
            0,
        )
        .expect_err("different owner should conflict with owner 10");
    assert_lock_conflict(conflict, "owner 20 conflict across handles");

    fs.release(ctx(), db.inode, 0, first_handle, false, false, None)
        .expect("release first handle");
    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            second_handle,
            20,
            fuse_byte_lock(libc::F_WRLCK, 0, 8),
            0,
        )
        .expect_err("releasing one handle must not drop owner 10 while another handle remains");
    assert_lock_conflict(conflict, "owner 20 after first handle release");

    fs.flush(ctx(), db.inode, second_handle, 10)
        .expect("flush owner 10");
    fs_setlk_eventually(
        &fs,
        db.inode,
        second_handle,
        20,
        fuse_byte_lock(libc::F_WRLCK, 0, 8),
    )
    .expect("owner 20 lock after owner 10 flush");
    fs.release(ctx(), db.inode, 0, second_handle, false, false, None)
        .expect("release second handle");
}

#[test]
fn composed_lock_bridge_flush_and_release_cleanup_owner_locks() {
    let test_dir = TestDir::new("lock-bridge-cleanup");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        10,
        fuse_byte_lock(libc::F_WRLCK, 0, 1),
        0,
    )
    .expect("owner 10 lock");
    fs.flush(ctx(), db.inode, handle, 10)
        .expect("flush owner 10");
    fs_setlk_eventually(
        &fs,
        db.inode,
        handle,
        20,
        fuse_byte_lock(libc::F_WRLCK, 0, 1),
    )
    .expect("owner 20 lock after flush");
    fs.release(ctx(), db.inode, 0, handle, false, false, None)
        .expect("release");

    let host = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("state.sqlite"))
        .expect("open host");
    fcntl_lock_eventually(
        host.as_raw_fd(),
        libc::F_SETLK,
        byte_lock(libc::F_WRLCK, 0, 1),
    )
    .expect("host POSIX lock after release");
}

#[test]
fn composed_lock_bridge_supports_shared_reads_and_write_exclusion() {
    let test_dir = TestDir::new("lock-bridge-shared-read");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_RDLCK, 0, 8),
        0,
    )
    .expect("owner 1 read lock");
    fs.setlk(
        ctx(),
        db.inode,
        handle,
        2,
        fuse_byte_lock(libc::F_RDLCK, 0, 8),
        0,
    )
    .expect("owner 2 read lock");
    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            handle,
            3,
            fuse_byte_lock(libc::F_WRLCK, 0, 8),
            0,
        )
        .expect_err("write lock should conflict with shared readers");
    assert_lock_conflict(conflict, "write over shared reads");
}

#[test]
fn composed_lock_bridge_preserves_locks_after_subrange_unlock() {
    let test_dir = TestDir::new("lock-bridge-subrange");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_WRLCK, 0, 10),
        0,
    )
    .expect("owner 1 write lock");
    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_UNLCK, 3, 2),
        0,
    )
    .expect("owner 1 subrange unlock");
    fs.setlk(
        ctx(),
        db.inode,
        handle,
        2,
        fuse_byte_lock(libc::F_WRLCK, 3, 2),
        0,
    )
    .expect("owner 2 lock in unlocked gap");
    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            handle,
            2,
            fuse_byte_lock(libc::F_WRLCK, 0, 1),
            0,
        )
        .expect_err("owner 1 should still hold bytes outside the unlocked gap");
    assert_lock_conflict(conflict, "lock outside subrange gap");
}

#[test]
fn composed_lock_bridge_setlkw_waits_for_unlock() {
    let test_dir = TestDir::new("lock-bridge-setlkw");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");
    let inode = db.inode;

    fs.setlk(
        ctx(),
        inode,
        handle,
        1,
        fuse_byte_lock(libc::F_WRLCK, 0, 1),
        0,
    )
    .expect("owner 1 write lock");
    let waiter_fs = fs.clone();
    let waiter = thread::spawn(move || {
        waiter_fs
            .setlkw(
                ctx(),
                inode,
                handle,
                2,
                fuse_byte_lock(libc::F_WRLCK, 0, 1),
                0,
            )
            .expect("blocking owner 2 lock")
    });
    thread::sleep(Duration::from_millis(50));
    assert!(!waiter.is_finished(), "SETLKW returned before unlock");
    fs.setlk(
        ctx(),
        inode,
        handle,
        1,
        fuse_byte_lock(libc::F_UNLCK, 0, 1),
        0,
    )
    .expect("owner 1 unlock");
    waiter.join().expect("waiter thread");
}

#[test]
fn composed_lock_bridge_setlkw_is_bounded_when_conflict_remains() {
    let test_dir = TestDir::new("lock-bridge-setlkw-bounded");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_WRLCK, 0, 1),
        0,
    )
    .expect("owner 1 write lock");
    let start = Instant::now();
    let error = fs
        .setlkw(
            ctx(),
            db.inode,
            handle,
            2,
            fuse_byte_lock(libc::F_WRLCK, 0, 1),
            0,
        )
        .expect_err("bounded setlkw conflict should return");
    assert_lock_conflict(error, "bounded setlkw conflict");
    assert!(
        start.elapsed() < SETLKW_MAX_WAIT * 4,
        "bounded setlkw waited too long: {:?}",
        start.elapsed()
    );

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_UNLCK, 0, 1),
        0,
    )
    .expect("owner 1 unlock after bounded wait");
}

#[test]
fn composed_lock_bridge_observes_host_held_posix_locks() {
    let test_dir = TestDir::new("lock-bridge-host-held");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    let host_path = root.join("state.sqlite");
    fs::write(&host_path, b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDWR as u32)
        .expect("open db");
    let handle = handle.expect("handle");
    let _host_lock = child_hold_posix_write_lock(&host_path, 0, 1);

    let conflict = fs
        .setlk(
            ctx(),
            db.inode,
            handle,
            1,
            fuse_byte_lock(libc::F_WRLCK, 0, 1),
            0,
        )
        .expect_err("guest write lock should conflict with host POSIX lock");
    assert_lock_conflict(conflict, "guest over host POSIX");
}

#[test]
fn composed_lock_bridge_respects_readonly_handles() {
    let test_dir = TestDir::new("lock-bridge-readonly");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("state.sqlite"), b"sqlite-lock-probe").expect("write db");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let db = lookup(&fs, workspace.inode, "state.sqlite").expect("lookup db");
    let (handle, _options) = fs
        .open(ctx(), db.inode, false, libc::O_RDONLY as u32)
        .expect("open db readonly");
    let handle = handle.expect("handle");

    fs.setlk(
        ctx(),
        db.inode,
        handle,
        1,
        fuse_byte_lock(libc::F_RDLCK, 0, 1),
        0,
    )
    .expect("read lock through readonly handle");
    assert_eq!(
        raw_error(
            fs.setlk(
                ctx(),
                db.inode,
                handle,
                1,
                fuse_byte_lock(libc::F_WRLCK, 0, 1),
                0,
            ),
            "write lock through readonly handle",
        ),
        Some(libc::EBADF)
    );
}

fn manifest_with_shadow_filter(
    host_root: &Path,
    shadow_root: &Path,
    access: AccessMode,
) -> Manifest {
    let mut manifest = manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        host_root,
        access,
    )]);
    manifest.shadow_root = Some(shadow_root.display().to_string());
    manifest.filters = vec![FilterSpec {
        mount_id: Some("workspace".to_string()),
        suffixes: vec!["-shm".to_string()],
        action: FilterAction::HideAndShadow,
    }];
    manifest
}

#[test]
fn filtered_shadow_hides_host_file_and_redirects_guest_writes() {
    let test_dir = TestDir::new("filtered-shadow-basic");
    let root = test_dir.join("root");
    let shadow = test_dir.join("shadow");
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("state.sqlite-shm"), b"host-shm").expect("host shm");
    fs::write(root.join("visible.txt"), b"visible").expect("visible");
    let namespace =
        Namespace::from_manifest(&manifest_with_shadow_filter(&root, &shadow, AccessMode::Rw))
            .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

    assert_eq!(
        raw_error(
            lookup(&fs, workspace.inode, "state.sqlite-shm"),
            "filtered host lookup",
        ),
        Some(libc::ENOENT)
    );
    let names = fs_readdir_names(&fs, workspace.inode, "").expect("readdir workspace");
    assert!(names.contains(&"visible.txt".to_string()));
    assert!(!names.contains(&"state.sqlite-shm".to_string()));

    fs_create_write_release(
        &fs,
        workspace.inode,
        "state.sqlite-shm",
        b"guest-shm".to_vec(),
    )
    .expect("write filtered shadow");

    assert_eq!(
        fs::read(root.join("state.sqlite-shm")).expect("host shm unchanged"),
        b"host-shm"
    );
    assert_eq!(
        fs::read(shadow.join("workspace/state.sqlite-shm")).expect("shadow shm"),
        b"guest-shm"
    );
    assert_eq!(
        fs_read_path(&fs, workspace.inode, "state.sqlite-shm").expect("read shadow"),
        b"guest-shm"
    );
    let names = fs_readdir_names(&fs, workspace.inode, "").expect("readdir workspace");
    assert!(names.contains(&"state.sqlite-shm".to_string()));
}

#[test]
fn filtered_shadow_respects_readonly_mounts() {
    let test_dir = TestDir::new("filtered-shadow-readonly");
    let root = test_dir.join("root");
    let shadow = test_dir.join("shadow");
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("state.sqlite-shm"), b"host-shm").expect("host shm");
    let namespace =
        Namespace::from_manifest(&manifest_with_shadow_filter(&root, &shadow, AccessMode::Ro))
            .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

    assert_eq!(
        raw_error(
            fs_create_write_release(
                &fs,
                workspace.inode,
                "state.sqlite-shm",
                b"guest-shm".to_vec(),
            ),
            "readonly filtered create",
        ),
        Some(libc::EROFS)
    );
    assert_eq!(
        fs::read(root.join("state.sqlite-shm")).expect("host shm unchanged"),
        b"host-shm"
    );
    assert!(!shadow.join("workspace/state.sqlite-shm").exists());
}

#[test]
fn filtered_shadow_rename_and_unlink_operate_on_shadow_only() {
    let test_dir = TestDir::new("filtered-shadow-rename");
    let root = test_dir.join("root");
    let shadow = test_dir.join("shadow");
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("a.sqlite-shm"), b"host-a").expect("host a");
    let namespace =
        Namespace::from_manifest(&manifest_with_shadow_filter(&root, &shadow, AccessMode::Rw))
            .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

    fs_create_write_release(&fs, workspace.inode, "a.sqlite-shm", b"guest-a".to_vec())
        .expect("write shadow a");
    fs_rename_path(&fs, workspace.inode, "a.sqlite-shm", "b.sqlite-shm")
        .expect("rename filtered shadow");

    assert_eq!(
        raw_error(
            lookup(&fs, workspace.inode, "a.sqlite-shm"),
            "old shadow lookup"
        ),
        Some(libc::ENOENT)
    );
    assert_eq!(
        fs_read_path(&fs, workspace.inode, "b.sqlite-shm").expect("read renamed shadow"),
        b"guest-a"
    );
    assert_eq!(
        fs::read(root.join("a.sqlite-shm")).expect("host a unchanged"),
        b"host-a"
    );
    assert!(!root.join("b.sqlite-shm").exists());

    fs_unlink_path(&fs, workspace.inode, "b.sqlite-shm").expect("unlink shadow b");
    assert_eq!(
        raw_error(
            lookup(&fs, workspace.inode, "b.sqlite-shm"),
            "unlinked shadow lookup"
        ),
        Some(libc::ENOENT)
    );
}

#[test]
fn filtered_shadow_rejects_crossing_between_host_and_shadow_on_rename() {
    let test_dir = TestDir::new("filtered-shadow-cross-rename");
    let root = test_dir.join("root");
    let shadow = test_dir.join("shadow");
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("visible.txt"), b"visible").expect("visible");
    let namespace =
        Namespace::from_manifest(&manifest_with_shadow_filter(&root, &shadow, AccessMode::Rw))
            .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");

    assert_eq!(
        raw_error(
            fs_rename_path(&fs, workspace.inode, "visible.txt", "state.sqlite-shm"),
            "rename host to shadow",
        ),
        Some(libc::EXDEV)
    );
    assert_eq!(
        fs::read(root.join("visible.txt")).expect("visible"),
        b"visible"
    );
    assert!(!root.join("state.sqlite-shm").exists());
    assert!(!shadow.join("workspace/state.sqlite-shm").exists());
}

#[test]
fn natural_home_overlay_merges_nested_workspace_and_tool_state_mounts() {
    let test_dir = TestDir::new("natural-home");
    let backing_home = test_dir.join("backing-home");
    let workspace = test_dir.join("workspace");
    let tool_state = test_dir.join("codex");
    fs::create_dir(&backing_home).expect("create home backing");
    fs::create_dir(&workspace).expect("create workspace");
    fs::create_dir(&tool_state).expect("create tool state");
    fs::write(backing_home.join(".profile"), b"home").expect("write home");
    fs::write(workspace.join("project.txt"), b"workspace").expect("write workspace");
    fs::write(tool_state.join("auth.json"), b"{}").expect("write tool state");
    let mut home_mount = dir_mount("home", "/home/user", &backing_home, AccessMode::Rw);
    home_mount.source_class = SourceClass::PersistentHome;
    let mut tool_mount = dir_mount("codex", "/home/user/.codex", &tool_state, AccessMode::Rw);
    tool_mount.source_class = SourceClass::ToolState;
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![
        home_mount,
        dir_mount(
            "workspace",
            "/home/user/project",
            &workspace,
            AccessMode::Rw,
        ),
        tool_mount,
    ]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let home = lookup(&fs, ROOT_ID, "home").expect("lookup home");
    let user = lookup(&fs, home.inode, "user").expect("lookup user");
    let project = lookup(&fs, user.inode, "project").expect("lookup project");
    let codex = lookup(&fs, user.inode, ".codex").expect("lookup tool state");

    assert!(lookup(&fs, user.inode, ".profile").is_ok());
    assert!(lookup(&fs, project.inode, "project.txt").is_ok());
    assert!(lookup(&fs, codex.inode, "auth.json").is_ok());
    assert!(backing_home.join(".profile").exists());
    assert!(workspace.join("project.txt").exists());
    assert!(tool_state.join("auth.json").exists());
}

#[test]
fn create_write_read_and_release_host_file() {
    let test_dir = TestDir::new("io");
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
fn host_and_guest_writes_are_visible_through_same_rw_mount() {
    let test_dir = TestDir::new("host-guest-rw");
    let root = test_dir.join("root");
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
    let name = CString::new("shared.txt").expect("name");
    let (entry, handle, _options) = fs
        .create(
            ctx(),
            workspace.inode,
            name.as_c_str(),
            0o644,
            false,
            libc::O_RDWR as u32,
            0,
            Extensions::default(),
        )
        .expect("create file");
    let handle = handle.expect("file handle");

    fs.write(
        ctx(),
        entry.inode,
        handle,
        VecReader {
            data: b"guest".to_vec(),
        },
        5,
        0,
        None,
        false,
        false,
        0,
    )
    .expect("guest write");
    fs.fsync(ctx(), entry.inode, false, handle).expect("fsync");
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).expect("host read"),
        "guest"
    );

    fs::write(root.join("shared.txt"), b"host-update").expect("host write");
    let mut reader = VecWriter::default();
    let read = fs
        .read(ctx(), entry.inode, handle, &mut reader, 64, 0, None, 0)
        .expect("guest read after host write");
    assert_eq!(read, 11);
    assert_eq!(reader.data, b"host-update");

    fs.release(ctx(), entry.inode, 0, handle, true, false, None)
        .expect("release");
}

#[test]
fn directory_link_rename_and_symlink_operations_use_host_tree() {
    let test_dir = TestDir::new("mutations");
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
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
fn open_beneath_masks_file_type_bits_from_create_mode() {
    let test_dir = TestDir::new("openat2-create-mode");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    let root = open_mount_root(&root, MountKind::Dir).expect("open mount root");

    let file = open_beneath_with_mode_impl(
        root.as_raw_fd(),
        Path::new("created"),
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
        libc::S_IFREG as u32 | 0o644,
        false,
    )
    .expect("create with FUSE file-type mode bits");
    drop(file);
    let metadata = stat_beneath(root.as_raw_fd(), Path::new("created")).expect("stat created");
    assert_eq!(metadata.mode() & 0o777, 0o644);
}

#[test]
fn open_beneath_fails_closed_when_openat2_is_unavailable() {
    let test_dir = TestDir::new("openat2-unavailable-fail-closed");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir(&root).expect("create root");
    fs::create_dir(&outside).expect("create outside");
    fs::write(outside.join("secret"), b"outside-secret").expect("write secret");
    symlink(&outside, root.join("link")).expect("create symlink");
    let root = open_mount_root(&root, MountKind::Dir).expect("open mount root");

    let error = open_beneath_with_mode_impl(
        root.as_raw_fd(),
        Path::new("link/secret"),
        libc::O_RDONLY,
        0,
        true,
    )
    .expect_err("openat fallback must not be used when openat2 is unavailable");
    assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP));
}

#[test]
fn host_symlink_swap_after_lookup_does_not_escape_mount_root() {
    let test_dir = TestDir::new("symlink-swap-after-lookup");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir(&root).expect("create root");
    fs::create_dir(&outside).expect("create outside");
    fs::write(root.join("victim"), b"inside").expect("write victim");
    fs::write(outside.join("secret"), b"outside-secret").expect("write secret");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let victim = lookup(&fs, workspace.inode, "victim").expect("lookup victim");

    fs::remove_file(root.join("victim")).expect("remove victim");
    symlink(outside.join("secret"), root.join("victim")).expect("replace with symlink");

    match fs.open(ctx(), victim.inode, false, libc::O_RDONLY as u32) {
        Ok((handle, _options)) => {
            let handle = handle.expect("handle");
            let mut reader = VecWriter::default();
            fs.read(ctx(), victim.inode, handle, &mut reader, 64, 0, None, 0)
                .expect("read swapped victim");
            assert_ne!(reader.data, b"outside-secret");
        }
        Err(error) => assert!(matches!(
            error.raw_os_error(),
            Some(libc::ENOENT | libc::EXDEV | libc::ELOOP | libc::ESTALE)
        )),
    }
}

#[test]
fn cached_child_under_host_parent_symlink_replacement_does_not_escape() {
    let test_dir = TestDir::new("parent-symlink-replace");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir_all(root.join("dir")).expect("create dir");
    fs::create_dir(&outside).expect("create outside");
    fs::write(root.join("dir/file"), b"inside").expect("write file");
    fs::write(outside.join("file"), b"outside-secret").expect("write secret");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let dir = lookup(&fs, workspace.inode, "dir").expect("lookup dir");
    let file = lookup(&fs, dir.inode, "file").expect("lookup file");

    fs::rename(root.join("dir"), root.join("moved")).expect("move dir");
    symlink(&outside, root.join("dir")).expect("replace parent with symlink");

    match fs.open(ctx(), file.inode, false, libc::O_RDONLY as u32) {
        Ok((handle, _options)) => {
            let handle = handle.expect("handle");
            let mut reader = VecWriter::default();
            fs.read(ctx(), file.inode, handle, &mut reader, 64, 0, None, 0)
                .expect("read cached child");
            assert_ne!(reader.data, b"outside-secret");
        }
        Err(error) => assert!(matches!(
            error.raw_os_error(),
            Some(libc::ENOENT | libc::EXDEV | libc::ELOOP | libc::ESTALE)
        )),
    }
}

#[test]
fn readlink_under_cached_parent_symlink_replacement_does_not_escape() {
    let test_dir = TestDir::new("readlink-parent-symlink-replace");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir_all(root.join("dir")).expect("create dir");
    fs::create_dir(&outside).expect("create outside");
    symlink("inside-target", root.join("dir/link")).expect("create inside link");
    symlink("outside-target", outside.join("link")).expect("create outside link");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let dir = lookup(&fs, workspace.inode, "dir").expect("lookup dir");
    let link = lookup(&fs, dir.inode, "link").expect("lookup link");

    fs::rename(root.join("dir"), root.join("moved")).expect("move dir");
    symlink(&outside, root.join("dir")).expect("replace dir with symlink");

    match fs.readlink(ctx(), link.inode) {
        Ok(target) => assert_ne!(target, b"outside-target"),
        Err(error) => assert!(matches!(
            error.raw_os_error(),
            Some(libc::ENOENT | libc::EXDEV | libc::ELOOP | libc::ENOTDIR | libc::ESTALE)
        )),
    }
}

#[test]
fn access_under_cached_parent_symlink_replacement_does_not_escape() {
    let test_dir = TestDir::new("access-parent-symlink-replace");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir_all(root.join("dir")).expect("create dir");
    fs::create_dir(&outside).expect("create outside");
    fs::write(root.join("dir/file"), b"inside").expect("write inside");
    fs::write(outside.join("file"), b"outside-secret").expect("write outside");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let dir = lookup(&fs, workspace.inode, "dir").expect("lookup dir");
    let file = lookup(&fs, dir.inode, "file").expect("lookup file");

    fs::rename(root.join("dir"), root.join("moved")).expect("move dir");
    symlink(&outside, root.join("dir")).expect("replace dir with symlink");

    let error = fs
        .access(ctx(), file.inode, libc::R_OK as u32)
        .expect_err("access must not probe through replaced parent symlink");
    assert!(matches!(
        error.raw_os_error(),
        Some(libc::ENOENT | libc::EXDEV | libc::ELOOP | libc::ENOTDIR | libc::ESTALE)
    ));
}

#[test]
fn cached_parent_lookup_after_host_symlink_replacement_does_not_escape() {
    let test_dir = TestDir::new("parent-lookup-symlink-replace");
    let root = test_dir.join("root");
    let outside = test_dir.join("outside");
    fs::create_dir_all(root.join("dir")).expect("create dir");
    fs::create_dir(&outside).expect("create outside");
    fs::write(root.join("dir/file"), b"inside").expect("write inside");
    fs::write(outside.join("file"), b"outside-secret").expect("write outside");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let dir = lookup(&fs, workspace.inode, "dir").expect("lookup dir");

    fs::rename(root.join("dir"), root.join("moved")).expect("move dir");
    symlink(&outside, root.join("dir")).expect("replace dir with symlink");

    match lookup(&fs, dir.inode, "file") {
        Ok(entry) => {
            let (handle, _options) = fs
                .open(ctx(), entry.inode, false, libc::O_RDONLY as u32)
                .expect("open looked-up child");
            let handle = handle.expect("handle");
            let mut reader = VecWriter::default();
            fs.read(ctx(), entry.inode, handle, &mut reader, 64, 0, None, 0)
                .expect("read looked-up child");
            assert_ne!(reader.data, b"outside-secret");
            fs.release(ctx(), entry.inode, 0, handle, false, false, None)
                .expect("release");
        }
        Err(error) => assert!(matches!(
            error.raw_os_error(),
            Some(libc::ENOENT | libc::EXDEV | libc::ELOOP | libc::ENOTDIR | libc::ESTALE)
        )),
    }
}

#[test]
fn concurrent_lookup_forget_churn_preserves_lookup_count() {
    let test_dir = TestDir::new("lookup-forget-churn");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("file"), b"data").expect("write file");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = Arc::new(ComposedFs::new(namespace));
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let file = lookup(&fs, workspace.inode, "file").expect("initial lookup");
    let baseline = lookup_count(&fs, file.inode);
    let workers = 4;
    let iterations = 32;
    let barrier = Arc::new(Barrier::new(workers));

    let threads = (0..workers)
        .map(|_| {
            let fs = Arc::clone(&fs);
            let barrier = Arc::clone(&barrier);
            let parent = workspace.inode;
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..iterations {
                    let entry = lookup(&fs, parent, "file").expect("thread lookup");
                    fs.forget(ctx(), entry.inode, 1);
                }
            })
        })
        .collect::<Vec<_>>();

    for thread in threads {
        thread.join().expect("lookup thread");
    }

    assert_eq!(lookup_count(&fs, file.inode), baseline);
    fs.forget(ctx(), file.inode, baseline);
}

#[test]
fn cross_mount_rename_and_link_return_exdev() {
    let test_dir = TestDir::new("cross-mount");
    let left = test_dir.join("left");
    let right = test_dir.join("right");
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
    let root = test_dir.join("root");
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
fn open_handle_survives_host_parent_rename_for_writes() {
    let test_dir = TestDir::new("open-parent-rename");
    let root = test_dir.join("root");
    fs::create_dir_all(root.join("dir")).expect("create dir");
    fs::write(root.join("dir/file"), b"hello").expect("write file");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let dir = lookup(&fs, workspace.inode, "dir").expect("lookup dir");
    let file = lookup(&fs, dir.inode, "file").expect("lookup file");
    let (handle, _options) = fs
        .open(ctx(), file.inode, false, libc::O_RDWR as u32)
        .expect("open file");
    let handle = handle.expect("handle");

    fs::rename(root.join("dir"), root.join("moved")).expect("host parent rename");
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
    .expect("write through open handle");
    fs.release(ctx(), file.inode, 0, handle, true, false, None)
        .expect("release");

    assert!(!root.join("dir").exists());
    assert_eq!(
        fs::read(root.join("moved/file")).expect("read moved file"),
        b"hello after"
    );
}

#[test]
fn stale_host_inode_does_not_follow_replacement_path() {
    let test_dir = TestDir::new("host-stale-replacement");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("file"), b"old").expect("write old");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let old = lookup(&fs, workspace.inode, "file").expect("lookup old");

    fs::remove_file(root.join("file")).expect("remove old");
    fs::write(root.join("file"), b"replacement").expect("write replacement");

    let replacement = lookup(&fs, workspace.inode, "file").expect("lookup replacement");
    assert_ne!(old.inode, replacement.inode);
    assert_eq!(
        raw_error(
            fs.open(ctx(), old.inode, false, libc::O_RDONLY as u32),
            "open stale old inode"
        ),
        Some(libc::ESTALE)
    );
    assert_eq!(
        raw_error(
            fs.getattr(ctx(), old.inode, None),
            "getattr stale old inode"
        ),
        Some(libc::ESTALE)
    );
    let (handle, _options) = fs
        .open(ctx(), replacement.inode, false, libc::O_RDONLY as u32)
        .expect("open replacement");
    let handle = handle.expect("handle");
    let mut reader = VecWriter::default();
    fs.read(
        ctx(),
        replacement.inode,
        handle,
        &mut reader,
        64,
        0,
        None,
        0,
    )
    .expect("read replacement");
    assert_eq!(reader.data, b"replacement");
    fs.release(ctx(), replacement.inode, 0, handle, false, false, None)
        .expect("release replacement");
}

#[test]
fn lookup_after_host_delete_recreate_reads_replacement() {
    let test_dir = TestDir::new("host-delete-recreate");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    fs::write(root.join("file"), b"old").expect("write old");
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let old = lookup(&fs, workspace.inode, "file").expect("lookup old");
    fs.forget(ctx(), old.inode, 1);

    fs::remove_file(root.join("file")).expect("remove old");
    fs::write(root.join("file"), b"replacement").expect("write replacement");

    let replacement = lookup(&fs, workspace.inode, "file").expect("lookup replacement");
    let (handle, _options) = fs
        .open(ctx(), replacement.inode, false, libc::O_RDONLY as u32)
        .expect("open replacement");
    let handle = handle.expect("handle");
    let mut reader = VecWriter::default();
    fs.read(
        ctx(),
        replacement.inode,
        handle,
        &mut reader,
        64,
        0,
        None,
        0,
    )
    .expect("read replacement");
    assert_eq!(reader.data, b"replacement");
}

#[test]
fn lookup_forget_counts_saturate_and_reject_malformed_components() {
    let test_dir = TestDir::new("lookup-forget");
    let root = test_dir.join("root");
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
    assert!(!namespace_contains_inode(&fs, first.inode));
    assert_eq!(lookup_count(&fs, ROOT_ID), root_lookup_count);
}

#[test]
fn readdir_prunes_cold_host_file_nodes() {
    let test_dir = TestDir::new("readdir-cold-prune");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    for index in 0..256 {
        fs::write(root.join(format!("file-{index:04}")), b"data").expect("write file");
    }
    for index in 0..16 {
        fs::create_dir(root.join(format!("dir-{index:04}"))).expect("create dir");
    }
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let baseline = namespace_cache_counts(&fs);

    let names = dir_names(
        fs.readdir(ctx(), workspace.inode, workspace.inode, 4096, 0)
            .expect("readdir workspace"),
    );
    assert_eq!(names.len(), readdir_entry_budget(4096));
    assert!(names.len() < 272);
    assert_eq!(namespace_cache_counts(&fs), baseline);

    let file = lookup(&fs, workspace.inode, "file-0007").expect("lookup file");
    assert!(namespace_contains_inode(&fs, file.inode));
    fs.forget(ctx(), file.inode, 1);
    assert!(!namespace_contains_inode(&fs, file.inode));
    assert_eq!(namespace_cache_counts(&fs), baseline);
}

#[test]
fn readdir_tiny_buffer_paginates_large_host_directory() {
    let test_dir = TestDir::new("readdir-tiny-pages");
    let root = test_dir.join("root");
    fs::create_dir(&root).expect("create root");
    let total = 96;
    for index in 0..total {
        fs::write(root.join(format!("file-{index:04}")), b"data").expect("write file");
    }
    let namespace = Namespace::from_manifest(&manifest_with_mounts(vec![dir_mount(
        "workspace",
        "/workspace",
        &root,
        AccessMode::Rw,
    )]))
    .expect("build namespace");
    let fs = ComposedFs::new(namespace);
    let workspace = lookup(&fs, ROOT_ID, "workspace").expect("lookup workspace");
    let baseline = namespace_cache_counts(&fs);
    let page_size = 256;
    let page_budget = readdir_entry_budget(page_size);
    assert!(page_budget < total);

    let mut offset = 0;
    let mut names = Vec::new();
    loop {
        let mut iter = fs
            .readdir(
                ctx(),
                workspace.inode,
                workspace.inode,
                page_size as u32,
                offset,
            )
            .expect("readdir page");
        let mut page = Vec::new();
        while let Some(entry) = iter.next() {
            offset = entry.offset;
            page.push(entry.name.to_str().expect("utf8").to_string());
        }
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= page_budget);
        names.extend(page);
        assert!(names.len() <= total, "pagination repeated entries");
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total);
    assert_eq!(namespace_cache_counts(&fs), baseline);
}

#[test]
fn release_with_wrong_inode_does_not_consume_valid_handle() {
    let test_dir = TestDir::new("release-wrong-inode");
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
    let first = String::from_utf8(first).expect("first utf8");
    let mut snapshot = vec![first];
    snapshot.extend(remaining);
    snapshot.sort();
    assert_eq!(snapshot, vec!["a".to_string(), "b".to_string()]);

    let mut fresh = dir_names(
        fs.readdir(ctx(), workspace.inode, workspace.inode, 4096, 0)
            .expect("fresh readdir"),
    );
    fresh.sort();
    assert_eq!(fresh, vec!["a".to_string(), "c".to_string()]);
}

#[test]
fn unsupported_device_mknod_fails_without_creating_host_node() {
    let test_dir = TestDir::new("unsupported-mknod");
    let root = test_dir.join("root");
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
fn proptest_nested_operation_sequences_cover_mount_boundaries() {
    let mut runner = TestRunner::new(Config {
        cases: 48,
        max_shrink_iters: 2048,
        failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
        ..Config::default()
    });
    runner
        .run(&nested_fs_ops(), |ops| {
            run_nested_fs_ops_case("proptest-nested-sequence", &ops);
            Ok(())
        })
        .expect("proptest nested operation sequence");
}

#[test]
#[ignore = "property stress: run explicitly with `cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_nested_operation_sequences_stress -- --ignored --nocapture`"]
fn proptest_nested_operation_sequences_stress() {
    let mut runner = TestRunner::new(Config {
        cases: 256,
        max_shrink_iters: 4096,
        failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
        ..Config::default()
    });
    runner
        .run(&nested_fs_ops(), |ops| {
            run_nested_fs_ops_case("proptest-nested-sequence-stress", &ops);
            Ok(())
        })
        .expect("stress proptest nested operation sequence");
}

#[test]
fn proptest_lock_operation_sequences_cover_owner_and_range_interleavings() {
    let mut runner = TestRunner::new(Config {
        cases: 96,
        max_shrink_iters: 4096,
        failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
        ..Config::default()
    });
    runner
        .run(&lock_stress_ops(96), |ops| {
            run_lock_ops_case("proptest-lock-sequence", &ops);
            Ok(())
        })
        .expect("proptest lock operation sequence");
}

#[test]
#[ignore = "property stress: run explicitly with `cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_lock_operation_sequences_stress -- --ignored --nocapture`"]
fn proptest_lock_operation_sequences_stress() {
    let mut runner = TestRunner::new(Config {
        cases: 512,
        max_shrink_iters: 8192,
        failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
        ..Config::default()
    });
    runner
        .run(&lock_stress_ops(320), |ops| {
            run_lock_ops_case("proptest-lock-sequence-stress", &ops);
            Ok(())
        })
        .expect("stress proptest lock operation sequence");
}

#[test]
#[ignore = "stress regression: run explicitly with `cargo test --manifest-path composed-fs/Cargo.toml --offline stress_seeded_flat_file_operation_sequences -- --ignored --nocapture`"]
fn stress_seeded_flat_file_operation_sequences() {
    const SEED: u64 = 0x5eed_f17e_2026_0514;
    const STEPS: usize = 512;
    let test_dir = TestDir::new("stress-flat-sequence");
    let root = test_dir.join("root");
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
    let root = test_dir.join("root");
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
