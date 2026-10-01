//! The little bit of libc the relay needs on Linux: who we are, where the
//! socket is, and who is on the other end of it.
//!
//! The socket sits in a directory only we can enter, so another account cannot
//! even reach it. Once connected we still ask the kernel (SO_PEERCRED) which
//! user is serving it, and say nothing to anybody but ourselves.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Must match `socket_dir()` in src-tauri/src/pipe.rs exactly.
pub fn socket_dir() -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    if run.is_dir() {
        return run.join("coucou");
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if dir.is_absolute() && dir.is_dir() {
            return dir.join("coucou");
        }
    }
    std::env::temp_dir().join(format!("coucou-{uid}"))
}

pub fn socket_path() -> PathBuf {
    socket_dir().join("hook.sock")
}

/// True when the process serving `stream` runs as the same user we do. A
/// failure to answer is treated as "not ours", as on Windows.
pub fn server_is_same_user(stream: &UnixStream) -> bool {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    rc == 0 && cred.pid != 0 && cred.uid == unsafe { libc::geteuid() }
}
