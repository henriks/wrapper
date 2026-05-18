use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rustix::fs::{Mode, OFlags, ResolveFlags};
use virtiofsd::filesystem::{GetxattrReply, ListxattrReply, SetattrValid};
use virtiofsd::fuse;
use virtiofsd::soft_idmap::Id;

use crate::manifest::MountKind;
use crate::state::FileHandle;
use crate::{
    invalid_input, validate_relative_path, validate_single_path_component, HostIdentity,
    MountRuntime, SETLKW_MAX_WAIT, SETLKW_RETRY_DELAY,
};

pub(super) fn open_mount_root(path: &Path, kind: MountKind) -> io::Result<File> {
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

pub(super) fn stat_beneath(root_fd: RawFd, relative_path: &Path) -> io::Result<fs::Metadata> {
    let file = open_beneath(root_fd, relative_path, libc::O_PATH | libc::O_NOFOLLOW)?;
    fstat_file(&file)
}

pub(super) fn open_beneath(root_fd: RawFd, relative_path: &Path, flags: i32) -> io::Result<File> {
    open_beneath_with_mode(root_fd, relative_path, flags, 0)
}

pub(super) fn open_beneath_for_io(
    root_fd: RawFd,
    relative_path: &Path,
    flags: i32,
    mode: u32,
) -> io::Result<File> {
    open_beneath_with_mode(root_fd, relative_path, flags, mode)
}

pub(super) fn open_host_file_for_io(
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

pub(super) fn validate_host_identity(
    metadata: &fs::Metadata,
    identity: HostIdentity,
) -> io::Result<()> {
    if identity.matches(metadata) {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::ESTALE))
    }
}

pub(super) fn reopen_lock_file(handle: &FileHandle) -> io::Result<File> {
    let flags = if handle.writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    let file = open_absolute_nofollow(&handle.lock_path, flags, 0)?;
    let metadata = fstat_file(&file)?;
    validate_host_identity(
        &metadata,
        HostIdentity {
            dev: handle.dev,
            ino: handle.ino,
        },
    )?;
    Ok(file)
}

pub(super) fn fuse_lock_to_host(lock: fuse::FileLock) -> io::Result<libc::flock> {
    let lock_type = match lock.type_ as i32 {
        libc::F_RDLCK => libc::F_RDLCK,
        libc::F_WRLCK => libc::F_WRLCK,
        libc::F_UNLCK => libc::F_UNLCK,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    if lock.end != u64::MAX && lock.end < lock.start {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let start = libc::off_t::try_from(lock.start)
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let len = if lock.end == u64::MAX {
        0
    } else {
        let len = lock
            .end
            .checked_sub(lock.start)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        libc::off_t::try_from(len).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?
    };
    Ok(libc::flock {
        l_type: lock_type as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: start,
        l_len: len,
        // Linux requires l_pid to be zero for F_OFD_SETLK/F_OFD_SETLKW input.
        l_pid: 0,
    })
}

pub(super) fn host_lock_to_fuse(lock: libc::flock) -> io::Result<fuse::FileLock> {
    let lock_type = match lock.l_type as i32 {
        libc::F_RDLCK => libc::F_RDLCK as u32,
        libc::F_WRLCK => libc::F_WRLCK as u32,
        libc::F_UNLCK => libc::F_UNLCK as u32,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    let start =
        u64::try_from(lock.l_start).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let end = if lock.l_len == 0 {
        u64::MAX
    } else {
        let len =
            u64::try_from(lock.l_len).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        start
            .checked_add(len)
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?
    };
    Ok(fuse::FileLock {
        start,
        end,
        type_: lock_type,
        pid: u32::try_from(lock.l_pid).unwrap_or(0),
    })
}

pub(super) fn fcntl_ofd_lock(
    fd: RawFd,
    command: i32,
    mut lock: libc::flock,
) -> io::Result<libc::flock> {
    // SAFETY: fcntl is called with a valid fd and a pointer to an initialized flock.
    let result = unsafe { libc::fcntl(fd, command, &mut lock) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(lock)
    }
}

pub(super) fn fcntl_ofd_lock_wait_bounded(fd: RawFd, lock: libc::flock) -> io::Result<libc::flock> {
    let deadline = Instant::now() + SETLKW_MAX_WAIT;
    loop {
        match fcntl_ofd_lock(fd, libc::F_OFD_SETLK, lock) {
            Ok(lock) => return Ok(lock),
            Err(error) if is_lock_conflict(&error) && Instant::now() < deadline => {
                std::thread::sleep(SETLKW_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn is_lock_conflict(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if code == libc::EACCES || code == libc::EAGAIN)
}

pub(super) fn open_absolute_nofollow(path: &Path, flags: i32, mode: u32) -> io::Result<File> {
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

pub(super) fn open_beneath_with_mode(
    root_fd: RawFd,
    relative_path: &Path,
    flags: i32,
    mode: u32,
) -> io::Result<File> {
    open_beneath_with_mode_impl(root_fd, relative_path, flags, mode, false)
}

pub(super) fn open_beneath_with_mode_impl(
    root_fd: RawFd,
    relative_path: &Path,
    flags: i32,
    mode: u32,
    force_openat2_unavailable: bool,
) -> io::Result<File> {
    validate_relative_path(relative_path)?;
    let path = if relative_path.as_os_str().is_empty() {
        CString::new(".")?
    } else {
        CString::new(relative_path.as_os_str().as_bytes())?
    };
    let root = unsafe { BorrowedFd::borrow_raw(root_fd) };
    let open_flags = OFlags::from_bits_retain((flags | libc::O_CLOEXEC) as _);
    let create_mode = Mode::from_bits_retain(mode & 0o7777);
    if force_openat2_unavailable {
        return Err(openat2_required_error());
    }
    match rustix::fs::openat2(
        root,
        path.as_c_str(),
        open_flags,
        create_mode,
        ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(fd) => Ok(File::from(fd)),
        Err(error) if matches!(error.raw_os_error(), libc::ENOSYS | libc::EINVAL) => {
            Err(openat2_required_error())
        }
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

pub(super) fn openat2_required_error() -> io::Error {
    io::Error::from_raw_os_error(libc::EOPNOTSUPP)
}

pub(super) fn open_flags_want_write(flags: u32) -> bool {
    let access_mode = flags as i32 & libc::O_ACCMODE;
    access_mode == libc::O_WRONLY
        || access_mode == libc::O_RDWR
        || flags & (libc::O_TRUNC as u32 | libc::O_APPEND as u32) != 0
}

pub(super) fn fstat_file(file: &File) -> io::Result<fs::Metadata> {
    file.metadata()
}

pub(super) fn parent_and_leaf(path: &Path) -> io::Result<(PathBuf, CString)> {
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

pub(super) fn mkdir_beneath(root_fd: RawFd, path: &Path, mode: u32) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::mkdirat(parent_fd, leaf.as_ptr(), mode) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn mknod_beneath(root_fd: RawFd, path: &Path, mode: u32, rdev: u32) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::mknodat(parent_fd, leaf.as_ptr(), mode, rdev.into()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn unlink_beneath(root_fd: RawFd, path: &Path, directory: bool) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
        let result = unsafe { libc::unlinkat(parent_fd, leaf.as_ptr(), flags) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn rename_beneath(
    root_fd: RawFd,
    old_path: &Path,
    new_path: &Path,
    flags: u32,
) -> io::Result<()> {
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

pub(super) fn link_beneath(root_fd: RawFd, old_path: &Path, new_path: &Path) -> io::Result<()> {
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

pub(super) fn symlink_beneath(root_fd: RawFd, linkname: &CStr, path: &Path) -> io::Result<()> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::symlinkat(linkname.as_ptr(), parent_fd, leaf.as_ptr()) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn readlink_beneath(root_fd: RawFd, path: &Path) -> io::Result<Vec<u8>> {
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let mut buffer = vec![0; 4096];
        let result = unsafe {
            libc::readlinkat(
                parent_fd,
                leaf.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(result as usize);
        Ok(buffer)
    })
}

pub(super) fn access_beneath(root_fd: RawFd, path: &Path, mask: i32) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        let current = c".";
        let result = unsafe { libc::faccessat(root_fd, current.as_ptr(), mask, libc::AT_EACCESS) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(());
    }
    with_parent_dir(root_fd, path, |parent_fd, leaf| {
        let result = unsafe { libc::faccessat(parent_fd, leaf.as_ptr(), mask, libc::AT_EACCESS) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn access_host_path(mount: &MountRuntime, path: &Path, mask: i32) -> io::Result<()> {
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

pub(super) fn fstatvfs_file(file: &File) -> io::Result<libc::statvfs64> {
    let mut st: libc::statvfs64 = unsafe { mem::zeroed() };
    let result = unsafe { libc::fstatvfs64(file.as_raw_fd(), &mut st) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

pub(super) fn setattr_wants_mutation(valid: SetattrValid) -> bool {
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

pub(super) fn apply_setattr(
    file: &File,
    attr: fuse::SetattrIn,
    valid: SetattrValid,
) -> io::Result<()> {
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

pub(super) fn setxattr_file(file: &File, name: &CStr, value: &[u8], flags: u32) -> io::Result<()> {
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

pub(super) fn getxattr_file(file: &File, name: &CStr, size: u32) -> io::Result<GetxattrReply> {
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

pub(super) fn listxattr_file(file: &File, size: u32) -> io::Result<ListxattrReply> {
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

pub(super) fn removexattr_file(file: &File, name: &CStr) -> io::Result<()> {
    let result = unsafe { libc::fremovexattr(file.as_raw_fd(), name.as_ptr()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn setattr_time(
    set_explicit: bool,
    set_now: bool,
    sec: u64,
    nsec: u32,
) -> libc::timespec {
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

pub(super) struct HostDirEntry {
    pub(super) name: String,
    pub(super) metadata: fs::Metadata,
}

pub(super) fn readdir_entry_budget(size: u32) -> usize {
    if size == 0 {
        return 0;
    }
    ((size as usize) / 64).clamp(1, 1024)
}

pub(super) fn readdir_collection_limit(offset: u64, entry_budget: usize) -> usize {
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    offset.saturating_add(entry_budget)
}

pub(super) fn read_dir_beneath(
    root_fd: RawFd,
    relative_path: &Path,
    limit: usize,
) -> io::Result<Vec<HostDirEntry>> {
    let dir = open_beneath(
        root_fd,
        relative_path,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )?;
    if limit == 0 {
        return Ok(Vec::new());
    }
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
        if entries.len() >= limit {
            break;
        }
    }
    Ok(entries)
}
