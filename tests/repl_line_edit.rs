//! REPL 行编辑：管道保持 `read_line`；Unix PTY 校验光标列与编辑结果。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Once;

fn cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("wjsm-test-cache").join("native");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn wjsm_bin() -> PathBuf {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if std::env::var_os("WJSM_CACHE_DIR").is_none() {
            // SAFETY: 测试初始化只设置一次共享 cache。
            unsafe {
                std::env::set_var("WJSM_CACHE_DIR", cache_dir());
            }
        }
    });
    std::env::var("CARGO_BIN_EXE_wjsm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/wjsm"))
}

#[test]
fn pipe_repl_keeps_readline_behavior() {
    let mut child = Command::new(wjsm_bin())
        .arg("repl")
        .env("WJSM_CACHE_DIR", cache_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wjsm repl");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"1+1\n.exit\n")
            .expect("write pipeline input");
    }
    let output = child.wait_with_output().expect("wait repl");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr={stderr}");
    assert!(stdout.contains('2'), "stdout={stdout}");
    assert!(
        !stdout.contains("wjsm>") && !stdout.contains("\x1b["),
        "pipeline must not emit prompt or CSI: {stdout:?}"
    );
}

#[cfg(unix)]
mod pty {
    use super::wjsm_bin;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const PROMPT: &str = "wjsm> ";
    const PROMPT_COLS: usize = 6;

    struct Session {
        master: File,
        child: Child,
        screen: Screen,
        raw: Vec<u8>,
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct Screen {
        pending: Vec<u8>,
        col: usize,
    }

    impl Screen {
        fn new() -> Self {
            Self {
                pending: Vec::new(),
                col: 0,
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            self.pending.extend_from_slice(bytes);
            self.drain();
        }

        fn drain(&mut self) {
            while !self.pending.is_empty() {
                match self.pending[0] {
                    b'\r' => {
                        self.col = 0;
                        self.pending.remove(0);
                    }
                    b'\n' => {
                        self.col = 0;
                        self.pending.remove(0);
                    }
                    0x08 => {
                        self.col = self.col.saturating_sub(1);
                        self.pending.remove(0);
                    }
                    0x1b => {
                        if !self.take_esc() {
                            return;
                        }
                    }
                    first => {
                        let Some(needed) = utf8_len(first) else {
                            self.pending.remove(0);
                            continue;
                        };
                        if self.pending.len() < needed {
                            return;
                        }
                        let bytes: Vec<u8> = self.pending.drain(..needed).collect();
                        if let Ok(text) = std::str::from_utf8(&bytes)
                            && let Some(ch) = text.chars().next()
                        {
                            self.col += char_cols(ch);
                        }
                    }
                }
            }
        }

        fn take_esc(&mut self) -> bool {
            if self.pending.len() < 2 {
                return false;
            }
            match self.pending[1] {
                b'[' => self.take_csi(),
                b'O' => {
                    if self.pending.len() < 3 {
                        return false;
                    }
                    self.pending.drain(..3);
                    true
                }
                _ => {
                    self.pending.remove(0);
                    true
                }
            }
        }

        fn take_csi(&mut self) -> bool {
            let Some(end) = self
                .pending
                .iter()
                .enumerate()
                .skip(2)
                .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
                .map(|(index, _)| index)
            else {
                return false;
            };
            let seq: Vec<u8> = self.pending.drain(..=end).collect();
            apply_csi(&mut self.col, &seq[2..]);
            true
        }
    }

    fn apply_csi(col: &mut usize, body: &[u8]) {
        let Some((&final_byte, params)) = body.split_last() else {
            return;
        };
        let count = atoi(params).unwrap_or(1);
        match final_byte {
            b'C' => *col += count,
            b'D' => *col = col.saturating_sub(count),
            b'G' => *col = count.saturating_sub(1),
            _ => {}
        }
    }

    fn atoi(bytes: &[u8]) -> Option<usize> {
        if bytes.is_empty() || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        std::str::from_utf8(bytes).ok()?.parse().ok()
    }

    fn utf8_len(first: u8) -> Option<usize> {
        if first < 0x80 {
            Some(1)
        } else if first & 0xe0 == 0xc0 {
            Some(2)
        } else if first & 0xf0 == 0xe0 {
            Some(3)
        } else if first & 0xf8 == 0xf0 {
            Some(4)
        } else {
            None
        }
    }

    fn char_cols(ch: char) -> usize {
        match ch {
            '\u{0301}' | '\u{200d}' | '\u{fe0f}' => 0,
            '中' | '你' | '👋' => 2,
            ch if ch.is_control() => 0,
            _ => 1,
        }
    }

    fn open_pty() -> (File, File) {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0, "posix_openpt: {}", io::Error::last_os_error());
            assert_eq!(libc::grantpt(master), 0);
            assert_eq!(libc::unlockpt(master), 0);
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(
                libc::ptsname_r(master, name.as_mut_ptr(), name.len()),
                0,
                "ptsname_r: {}",
                io::Error::last_os_error()
            );
            let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(slave >= 0, "open slave: {}", io::Error::last_os_error());
            let winsize = libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            libc::ioctl(master, libc::TIOCSWINSZ, &winsize);
            libc::ioctl(slave, libc::TIOCSWINSZ, &winsize);
            let flags = libc::fcntl(master, libc::F_GETFL);
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
            (File::from_raw_fd(master), File::from_raw_fd(slave))
        }
    }

    fn spawn_repl() -> Session {
        let (master, slave) = open_pty();
        let master_fd = master.as_raw_fd();
        let slave_fd = slave.as_raw_fd();
        let slave_in = slave.try_clone().expect("slave stdin");
        let slave_out = slave.try_clone().expect("slave stdout");
        let slave_err = slave.try_clone().expect("slave stderr");
        let child = unsafe {
            Command::new(wjsm_bin())
                .arg("repl")
                .env("WJSM_CACHE_DIR", super::cache_dir())
                .env("TERM", "xterm")
                .stdin(Stdio::from(slave_in))
                .stdout(Stdio::from(slave_out))
                .stderr(Stdio::from(slave_err))
                .pre_exec(move || {
                    libc::setsid();
                    libc::ioctl(slave_fd, libc::TIOCSCTTY, 0);
                    libc::close(master_fd);
                    Ok(())
                })
                .spawn()
                .expect("spawn pty repl")
        };
        drop(slave);
        let mut session = Session {
            master,
            child,
            screen: Screen::new(),
            raw: Vec::new(),
        };
        session.wait_contains(PROMPT, Duration::from_secs(8));
        session
    }

    impl Session {
        fn poll_read(&mut self) {
            let mut buf = [0u8; 256];
            match self.master.read(&mut buf) {
                Ok(0) => {}
                Ok(count) => {
                    self.raw.extend_from_slice(&buf[..count]);
                    self.screen.feed(&buf[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("master read: {error}"),
            }
        }

        fn wait_contains(&mut self, needle: &str, timeout: Duration) {
            self.wait_since(needle, 0, timeout);
        }

        fn wait_since(&mut self, needle: &str, since: usize, timeout: Duration) -> usize {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                self.poll_read();
                let text = String::from_utf8_lossy(&self.raw);
                if let Some(offset) = text.get(since..).and_then(|tail| tail.find(needle)) {
                    return since + offset + needle.len();
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!(
                "timeout waiting for {needle:?} since {since} in {:?}",
                String::from_utf8_lossy(&self.raw)
            );
        }

        fn wait_col(&mut self, column: usize, timeout: Duration) {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                self.poll_read();
                if self.screen.col == column {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!("timeout waiting for col {column}: {}", self.dump());
        }

        fn write_all(&mut self, bytes: &[u8]) {
            self.master.write_all(bytes).expect("write master");
            self.master.flush().ok();
        }

        fn dump(&self) -> String {
            format!(
                "col={} raw={:?}",
                self.screen.col,
                String::from_utf8_lossy(&self.raw)
            )
        }
    }

    #[test]
    fn pty_cjk_emoji_combining_cursor_columns() {
        let mut session = spawn_repl();
        assert_eq!(session.screen.col, PROMPT_COLS, "{}", session.dump());

        session.write_all("中".as_bytes());
        session.wait_contains("中", Duration::from_secs(2));
        session.wait_col(PROMPT_COLS + 2, Duration::from_secs(2));

        session.write_all(b"\x1b[D");
        session.wait_col(PROMPT_COLS, Duration::from_secs(2));

        session.write_all(b"\x1b[C\x7f");
        session.wait_col(PROMPT_COLS, Duration::from_secs(2));

        session.write_all("👋".as_bytes());
        session.wait_contains("👋", Duration::from_secs(2));
        session.wait_col(PROMPT_COLS + 2, Duration::from_secs(2));
        session.write_all(b"\x7f");
        session.wait_col(PROMPT_COLS, Duration::from_secs(2));

        session.write_all("e\u{0301}".as_bytes());
        session.wait_contains("e", Duration::from_secs(2));
        session.wait_col(PROMPT_COLS + 1, Duration::from_secs(2));
        session.write_all(b"\x1b[D");
        session.wait_col(PROMPT_COLS, Duration::from_secs(2));
    }

    #[test]
    fn pty_left_insert_then_eval_and_history() {
        let mut session = spawn_repl();
        let mut at = session.raw.len();
        session.write_all(b"\"");
        session.write_all("你".as_bytes());
        session.write_all(b"\"\x1b[D\x1b[Dx\r");
        at = session.wait_since("x你", at, Duration::from_secs(8));
        at = session.wait_since(PROMPT, at, Duration::from_secs(8));

        session.write_all(b"1+1\r");
        at = session.wait_since("2", at, Duration::from_secs(8));
        at = session.wait_since(PROMPT, at, Duration::from_secs(8));
        session.write_all(b"\x1b[A\r");
        session.wait_since("2", at, Duration::from_secs(8));
    }
}
