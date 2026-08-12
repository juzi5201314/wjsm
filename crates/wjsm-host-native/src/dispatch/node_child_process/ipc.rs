use std::collections::VecDeque;
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct IpcMessage {
    pub(super) payload: String,
    pub(super) fd: Option<RawFd>,
}

pub(super) struct IpcEndpoint {
    fd: RawFd,
    owned: Mutex<Option<OwnedFd>>,
    closed: AtomicBool,
    inbox: Mutex<VecDeque<IpcMessage>>,
    writer: Mutex<()>,
    socket_path: Option<PathBuf>,
}

impl IpcEndpoint {
    fn connected(fd: RawFd, socket_path: Option<PathBuf>) -> io::Result<Arc<Self>> {
        // SAFETY: fd 来自 into_raw_fd，所有权在此处唯一转入 OwnedFd。
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let endpoint = Arc::new(Self {
            fd: owned.as_raw_fd(),
            owned: Mutex::new(Some(owned)),
            closed: AtomicBool::new(false),
            inbox: Mutex::new(VecDeque::new()),
            writer: Mutex::new(()),
            socket_path,
        });
        Self::start_reader(&endpoint)?;
        Ok(endpoint)
    }

    fn start_reader(endpoint: &Arc<Self>) -> io::Result<()> {
        let endpoint = Arc::clone(endpoint);
        thread::Builder::new()
            .name("wjsm-ipc-reader".into())
            .spawn(move || {
                while !endpoint.is_closed() {
                    match recv_framed(endpoint.fd) {
                        Ok(message) => endpoint
                            .inbox
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push_back(message),
                        Err(_) => break,
                    }
                }
                endpoint.closed.store(true, Ordering::Release);
            })?;
        Ok(())
    }

    pub(super) fn send(&self, payload: &str, fd: Option<RawFd>) -> io::Result<()> {
        if self.is_closed() {
            return Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "IPC channel is closed",
            ));
        }
        let _writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        send_framed(self.fd, payload.as_bytes(), fd)
    }

    pub(super) fn pop(&self) -> Option<IpcMessage> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(super) fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: fd 在 owned 存活期间有效；shutdown 只终止双向流。
        unsafe { libc::shutdown(self.fd, libc::SHUT_RDWR) };
        self.owned
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

impl Drop for IpcEndpoint {
    fn drop(&mut self) {
        self.close();
        if let Some(path) = &self.socket_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(Clone)]
pub(super) struct ParentIpcHandle {
    path: PathBuf,
    inner: Arc<ParentIpcInner>,
}

struct ParentIpcInner {
    endpoint: Mutex<Option<Arc<IpcEndpoint>>>,
    pending: Mutex<Vec<(String, Option<RawFd>)>>,
    error: Mutex<Option<String>>,
}

impl ParentIpcHandle {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn endpoint(&self) -> Option<Arc<IpcEndpoint>> {
        self.inner
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn send(&self, payload: String, fd: Option<RawFd>) -> io::Result<()> {
        if let Some(error) = self
            .inner
            .error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Err(io::Error::other(error));
        }
        if let Some(endpoint) = self.endpoint() {
            return endpoint.send(&payload, fd);
        }
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((payload, fd));
        if let Some(endpoint) = self.endpoint() {
            self.flush_pending(&endpoint);
        }
        Ok(())
    }

    fn publish(&self, endpoint: Arc<IpcEndpoint>) {
        *self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&endpoint));
        self.flush_pending(&endpoint);
    }

    fn flush_pending(&self, endpoint: &IpcEndpoint) {
        let pending = std::mem::take(
            &mut *self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for (payload, fd) in pending {
            let _ = endpoint.send(&payload, fd);
        }
    }
}

pub(super) fn create_parent() -> io::Result<ParentIpcHandle> {
    let path = std::env::temp_dir().join(format!(
        "wjsm-ipc-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    let handle = ParentIpcHandle {
        path: path.clone(),
        inner: Arc::new(ParentIpcInner {
            endpoint: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            error: Mutex::new(None),
        }),
    };
    let accept_handle = handle.clone();
    thread::Builder::new()
        .name("wjsm-ipc-accept".into())
        .spawn(move || {
            let started = Instant::now();
            let accepted = loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        break IpcEndpoint::connected(stream.into_raw_fd(), Some(path));
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        if started.elapsed() >= Duration::from_secs(15) {
                            break Err(io::Error::new(ErrorKind::TimedOut, "IPC accept timed out"));
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => break Err(error),
                }
            };
            match accepted {
                Ok(endpoint) => accept_handle.publish(endpoint),
                Err(error) => {
                    *accept_handle
                        .inner
                        .error
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.to_string());
                }
            }
        })?;
    Ok(handle)
}

pub(super) fn connect(path: &str) -> io::Result<Arc<IpcEndpoint>> {
    let started = Instant::now();
    loop {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(stream) => return IpcEndpoint::connected(stream.into_raw_fd(), None),
            Err(error) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(2));
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
}

fn send_framed(socket: RawFd, payload: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "IPC payload is too large"))?;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(payload);
    let mut written = 0;
    while written < frame.len() {
        let count = if written == 0 {
            send_chunk(socket, &frame, fd)?
        } else {
            write_chunk(socket, &frame[written..])?
        };
        if count == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "IPC write returned zero",
            ));
        }
        written += count;
    }
    Ok(())
}

fn send_chunk(socket: RawFd, frame: &[u8], fd: Option<RawFd>) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: frame.as_ptr().cast_mut().cast(),
        iov_len: frame.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    let space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) as usize };
    let mut control = fd.map(|_| vec![0_u8; space]);
    if let (Some(fd), Some(control)) = (fd, control.as_mut()) {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        // SAFETY: control 按 CMSG_SPACE 分配，message 指向其完整可写区。
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
            std::ptr::write(libc::CMSG_DATA(header).cast::<RawFd>(), fd);
        }
    }
    // SAFETY: message 中所有指针在调用期间有效。
    let result = unsafe { libc::sendmsg(socket, &message, 0) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result as usize)
    }
}

fn write_chunk(socket: RawFd, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: bytes 指针和长度有效，socket 由 endpoint 拥有。
    let result = unsafe { libc::write(socket, bytes.as_ptr().cast(), bytes.len()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result as usize)
    }
}

fn recv_framed(socket: RawFd) -> io::Result<IpcMessage> {
    let (length, fd) = recv_header(socket)?;
    let mut payload = vec![0_u8; length as usize];
    read_exact(socket, &mut payload)?;
    let payload = String::from_utf8(payload)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    Ok(IpcMessage { payload, fd })
}

fn recv_header(socket: RawFd) -> io::Result<(u32, Option<RawFd>)> {
    let mut header = [0_u8; 4];
    let mut iov = libc::iovec {
        iov_base: header.as_mut_ptr().cast(),
        iov_len: header.len(),
    };
    let space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) as usize };
    let mut control = vec![0_u8; space];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len();
    // SAFETY: message 中的 header/control 缓冲区均在调用期间有效。
    let count = unsafe { libc::recvmsg(socket, &mut message, 0) };
    if count <= 0 {
        return Err(if count == 0 {
            io::Error::new(ErrorKind::UnexpectedEof, "IPC channel reached EOF")
        } else {
            io::Error::last_os_error()
        });
    }
    let fd = received_fd(&message);
    if count < 4 {
        read_exact(socket, &mut header[count as usize..])?;
    }
    Ok((u32::from_le_bytes(header), fd))
}

fn received_fd(message: &libc::msghdr) -> Option<RawFd> {
    // SAFETY: message 来自成功 recvmsg，control 区由 libc 填充。
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                return Some(std::ptr::read(libc::CMSG_DATA(header).cast::<RawFd>()));
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    None
}

fn read_exact(socket: RawFd, bytes: &mut [u8]) -> io::Result<()> {
    let mut read = 0;
    while read < bytes.len() {
        // SAFETY: 未初始化尾部为有效可写缓冲区，socket 由 endpoint 拥有。
        let count = unsafe {
            libc::read(
                socket,
                bytes[read..].as_mut_ptr().cast(),
                bytes.len() - read,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "IPC channel reached EOF",
            ));
        }
        read += count as usize;
    }
    Ok(())
}
