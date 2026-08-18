//! Unix raw mode：进入时关掉 ICANON/ECHO/ISIG，退出时恢复。

use std::io;

#[cfg(unix)]
use std::os::fd::AsRawFd;

/// 持有原始 termios；`Drop` 时恢复。
pub(super) struct RawModeGuard {
    #[cfg(unix)]
    fd: libc::c_int,
    #[cfg(unix)]
    original: libc::termios,
}

impl RawModeGuard {
    pub(super) fn enter() -> io::Result<Self> {
        #[cfg(unix)]
        {
            enter_unix()
        }
        #[cfg(not(unix))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "raw mode is only available on Unix",
            ))
        }
    }
}

#[cfg(unix)]
fn enter_unix() -> io::Result<RawModeGuard> {
    let fd = io::stdin().as_raw_fd();
    let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: fd 是 stdin，original 指向有效 termios。
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut raw = original;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
    raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    // SAFETY: raw 是刚刚从同一 fd 复制并改过的 termios。
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(RawModeGuard { fd, original })
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // SAFETY: fd 与 original 来自成功的 enter_unix。
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}
