//! Validation for inherited, same-user local credential channels.

use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;

/// Takes ownership only after verifying that the descriptor is a connected
/// local socket. A credential helper never accepts stdout, a file, or TCP.
pub(crate) fn inherited_socket(fd: i32) -> io::Result<UnixStream> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid credential channel",
        )
    };
    unsafe {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat(fd, metadata.as_mut_ptr()) != 0
            || metadata.assume_init().st_mode & libc::S_IFMT != libc::S_IFSOCK
        {
            return Err(invalid());
        }
    }
    let channel = unsafe { UnixStream::from_raw_fd(fd) };
    unsafe {
        let mut address = std::mem::MaybeUninit::<libc::sockaddr_storage>::uninit();
        let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getpeername(fd, address.as_mut_ptr().cast(), &mut length) != 0
            || address.assume_init().ss_family as i32 != libc::AF_UNIX
        {
            return Err(invalid());
        }
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut len,
        ) != 0
            || credentials.assume_init().uid != libc::getuid()
        {
            return Err(invalid());
        }
    }
    #[cfg(not(target_os = "linux"))]
    unsafe {
        let (mut uid, mut gid) = (0, 0);
        if libc::getpeereid(fd, &mut uid, &mut gid) != 0 || uid != libc::getuid() {
            return Err(invalid());
        }
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(channel)
}
