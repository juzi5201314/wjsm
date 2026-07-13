//! 子进程 IPC：Unix stream + 长度前缀帧 + 可选 SCM_RIGHTS 传 fd。
//!
//! 不依赖 spawn 继承 fd（Rust Command 会 close_range 非 stdio fd）。
//! 改用临时 Unix socket 路径：parent listen，child connect，路径经 env 传递。

use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::scheduler::AsyncHostCompletion;

/// 构造 owner 线程上的 wake 任务（IPC reader 线程通过 channel 投递）。
pub(crate) type IpcWakeTaskFactory = Arc<
    dyn Fn() -> (
            Box<dyn FnOnce(&mut wasmtime::Store<crate::RuntimeState>, &crate::WasmEnv) + Send>,
            Option<crate::CapturedScope>,
        ) + Send
        + Sync,
>;

/// 单条 IPC 消息：文本载荷 + 可选 ancillary fd。
#[derive(Debug)]
pub(crate) struct IpcMessage {
    pub payload: String,
    pub fd: Option<RawFd>,
}

/// 一端 IPC 通道（parent 或 child）。
pub(crate) struct IpcEndpoint {
    pub(crate) fd: RawFd,
    _owned: Mutex<Option<OwnedFd>>,
    closed: AtomicBool,
    reader: Mutex<Option<JoinHandle<()>>>,
    wake_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<AsyncHostCompletion>>>,
    inbox: Mutex<Vec<IpcMessage>>,
    reading: AtomicBool,
    /// 若为 parent 侧临时 socket 路径，drop 时删除。
    sock_path: Option<PathBuf>,
}

impl IpcEndpoint {
    fn from_raw_connected(fd: RawFd, sock_path: Option<PathBuf>) -> io::Result<Self> {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self {
            fd: owned.as_raw_fd(),
            _owned: Mutex::new(Some(owned)),
            closed: AtomicBool::new(false),
            reader: Mutex::new(None),
            wake_tx: Mutex::new(None),
            inbox: Mutex::new(Vec::new()),
            reading: AtomicBool::new(false),
            sock_path,
        })
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn set_wake_tx(
        &self,
        tx: Option<tokio::sync::mpsc::UnboundedSender<AsyncHostCompletion>>,
    ) {
        *self.wake_tx.lock().unwrap_or_else(|e| e.into_inner()) = tx;
    }

    pub(crate) fn send(&self, payload: &str, send_fd: Option<RawFd>) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(ErrorKind::BrokenPipe, "ipc closed"));
        }
        let bytes = payload.as_bytes();
        if bytes.len() > u32::MAX as usize {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "ipc payload too large",
            ));
        }
        send_framed(self.fd, bytes, send_fd)
    }

    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            libc::shutdown(self.fd, libc::SHUT_RDWR);
        }
        let _ = self._owned.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(path) = &self.sock_path {
            let _ = std::fs::remove_file(path);
        }
    }

    pub(crate) fn ensure_reader(self: &Arc<Self>, make_wake_task: IpcWakeTaskFactory) {
        if self.reading.swap(true, Ordering::SeqCst) {
            return;
        }
        let endpoint = Arc::clone(self);
        let fd = self.fd;
        let handle = thread::Builder::new()
            .name("wjsm-ipc-reader".into())
            .spawn(move || {
                loop {
                    if endpoint.closed.load(Ordering::SeqCst) {
                        break;
                    }
                    let msg = match recv_framed(fd) {
                        Ok(msg) => msg,
                        Err(_) => break,
                    };
                    endpoint
                        .inbox
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(msg);
                    wake_with(&endpoint, &make_wake_task);
                }
                endpoint.closed.store(true, Ordering::SeqCst);
                wake_with(&endpoint, &make_wake_task);
            })
            .expect("spawn ipc reader");
        *self.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }

    pub(crate) fn drain_inbox(&self) -> Vec<IpcMessage> {
        self.inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

impl Drop for IpcEndpoint {
    fn drop(&mut self) {
        self.close();
    }
}

/// Parent 侧：listen 后异步 accept；send 不阻塞 host（未就绪则入队）。
#[derive(Clone)]
pub(crate) struct ParentIpcHandle {
    path: PathBuf,
    inner: Arc<ParentIpcInner>,
}

struct ParentIpcInner {
    endpoint: Mutex<Option<Arc<IpcEndpoint>>>,
    pending: Mutex<Vec<(String, Option<RawFd>)>>,
    error: Mutex<Option<String>>,
    /// accept 完成（endpoint 或 error）时唤醒 wait_endpoint。
    ready: Condvar,
}

impl ParentIpcHandle {
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 非阻塞取已就绪 endpoint（不 wait）。
    pub(crate) fn try_endpoint(&self) -> Option<Arc<IpcEndpoint>> {
        self.inner
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 非阻塞：已就绪则发送；否则排队，accept 后刷出。
    /// 与 set_endpoint 并发时通过「push 后再检查 endpoint」避免丢消息。
    pub(crate) fn send_nonblocking(
        &self,
        payload: String,
        send_fd: Option<RawFd>,
    ) -> io::Result<()> {
        if let Some(err) = self
            .inner
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(io::Error::other(err));
        }
        if let Some(ep) = self.try_endpoint() {
            return ep.send(&payload, send_fd);
        }
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((payload, send_fd));
        // 竞态：accept 可能在 push 之前完成并已 drain 过空 pending
        if let Some(ep) = self.try_endpoint() {
            let leftover =
                std::mem::take(&mut *self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()));
            for (p, fd) in leftover {
                let _ = ep.send(&p, fd);
            }
        }
        Ok(())
    }

    /// 阻塞 wait（仅后台线程使用，绝不能在 host 调用路径上调用）。
    pub(crate) fn wait_endpoint(&self) -> io::Result<Arc<IpcEndpoint>> {
        let mut guard = self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(ep) = guard.as_ref() {
                return Ok(Arc::clone(ep));
            }
            if let Some(err) = self
                .inner
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return Err(io::Error::other(err));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(ErrorKind::TimedOut, "ipc accept timeout"));
            }
            let (next, wait_result) = self
                .inner
                .ready
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
            if wait_result.timed_out() {
                if let Some(ep) = guard.as_ref() {
                    return Ok(Arc::clone(ep));
                }
                if let Some(err) = self
                    .inner
                    .error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                {
                    return Err(io::Error::other(err));
                }
                return Err(io::Error::new(ErrorKind::TimedOut, "ipc accept timeout"));
            }
        }
    }

    fn set_endpoint(&self, ep: Arc<IpcEndpoint>) {
        // 先挂 endpoint 并唤醒 waiters，再循环 drain pending，避免与 send_nonblocking 丢消息
        {
            *self
                .inner
                .endpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&ep));
        }
        self.inner.ready.notify_all();
        loop {
            let batch =
                std::mem::take(&mut *self.inner.pending.lock().unwrap_or_else(|e| e.into_inner()));
            if batch.is_empty() {
                break;
            }
            for (payload, fd) in batch {
                let _ = ep.send(&payload, fd);
            }
        }
    }

    fn set_error(&self, err: String) {
        *self.inner.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(err);
        self.inner.ready.notify_all();
    }
}

pub(crate) fn create_parent_ipc() -> io::Result<ParentIpcHandle> {
    let path = std::env::temp_dir().join(format!(
        "wjsm-ipc-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    // 非阻塞 + poll：无连接时阻塞在 poll，而非 2ms 忙等
    listener.set_nonblocking(true)?;

    let handle = ParentIpcHandle {
        path: path.clone(),
        inner: Arc::new(ParentIpcInner {
            endpoint: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            ready: Condvar::new(),
        }),
    };
    let handle_t = handle.clone();
    let path_for_ep = path;

    thread::Builder::new()
        .name("wjsm-ipc-accept".into())
        .spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let result = loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        break IpcEndpoint::from_raw_connected(
                            stream.into_raw_fd(),
                            Some(path_for_ep),
                        )
                        .map(Arc::new);
                    }
                    Err(e)
                        if e.kind() == ErrorKind::WouldBlock
                            || e.kind() == ErrorKind::Interrupted =>
                    {
                        let now = Instant::now();
                        if now >= deadline {
                            break Err(io::Error::new(ErrorKind::TimedOut, "ipc accept timeout"));
                        }
                        let remaining_ms =
                            (deadline - now).as_millis().min(i32::MAX as u128) as libc::c_int;
                        let mut pfd = libc::pollfd {
                            fd: listener.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        let n = unsafe { libc::poll(&mut pfd, 1, remaining_ms) };
                        if n < 0 {
                            let err = io::Error::last_os_error();
                            if err.kind() == ErrorKind::Interrupted {
                                continue;
                            }
                            break Err(err);
                        }
                    }
                    Err(e) => break Err(e),
                }
            };
            match result {
                Ok(ep) => handle_t.set_endpoint(ep),
                Err(e) => handle_t.set_error(e.to_string()),
            }
        })
        .expect("ipc accept thread");

    Ok(handle)
}

/// Child：连接 parent 的 Unix socket 路径。
pub(crate) fn connect_ipc_path(path: &str) -> io::Result<Arc<IpcEndpoint>> {
    let start = std::time::Instant::now();
    let stream = loop {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(s) => break s,
            Err(e) => {
                if start.elapsed() > Duration::from_secs(5) {
                    return Err(e);
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    };
    let fd = stream.into_raw_fd();
    IpcEndpoint::from_raw_connected(fd, None).map(Arc::new)
}

fn wake_with(endpoint: &IpcEndpoint, make_wake_task: &IpcWakeTaskFactory) {
    if let Some(tx) = endpoint
        .wake_tx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        let (run, scope) = make_wake_task();
        let _ = tx.send(AsyncHostCompletion::HostTask { run, scope });
    }
}

fn send_framed(sock: RawFd, payload: &[u8], send_fd: Option<RawFd>) -> io::Result<()> {
    let len = (payload.len() as u32).to_le_bytes();
    if let Some(fd) = send_fd {
        if payload.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "ipc fd payload must not be empty",
            ));
        }
        write_all_fd(sock, &len)?;
        return send_with_fd(sock, payload, fd);
    }

    let mut data = Vec::with_capacity(4 + payload.len());
    data.extend_from_slice(&len);
    data.extend_from_slice(payload);
    write_all_fd(sock, &data)
}

fn write_all_fd(sock: RawFd, data: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < data.len() {
        let n = unsafe {
            libc::write(
                sock,
                data[written..].as_ptr() as *const libc::c_void,
                data.len() - written,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(ErrorKind::WriteZero, "ipc write zero"));
        }
        written += n as usize;
    }
    Ok(())
}

fn send_with_fd(sock: RawFd, data: &[u8], fd: RawFd) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize };
    let mut cmsg_buf = vec![0u8; cmsg_space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len() as _;

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::other("CMSG_FIRSTHDR null"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        let data_ptr = libc::CMSG_DATA(cmsg) as *mut RawFd;
        std::ptr::write(data_ptr, fd);
    }

    let n = unsafe { libc::sendmsg(sock, &msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != data.len() {
        return Err(io::Error::other("sendmsg short write"));
    }
    Ok(())
}

fn recv_framed(sock: RawFd) -> io::Result<IpcMessage> {
    let mut len_buf = [0u8; 4];
    read_exact_fd(sock, &mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    let fd = read_exact_fd_maybe_rights(sock, &mut payload)?;
    let text = String::from_utf8(payload).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
    Ok(IpcMessage { payload: text, fd })
}

fn read_exact_fd(sock: RawFd, buf: &mut [u8]) -> io::Result<()> {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::read(
                sock,
                buf[off..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - off,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "ipc eof"));
        }
        off += n as usize;
    }
    Ok(())
}

fn read_exact_fd_maybe_rights(sock: RawFd, buf: &mut [u8]) -> io::Result<Option<RawFd>> {
    let mut off = 0;
    let mut got_fd = None;
    while off < buf.len() {
        let mut iov = libc::iovec {
            iov_base: buf[off..].as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len() - off,
        };
        let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize };
        let mut cmsg_buf = vec![0u8; cmsg_space];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_buf.len() as _;

        let n = unsafe { libc::recvmsg(sock, &mut msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "ipc eof"));
        }
        off += n as usize;

        if got_fd.is_none() && msg.msg_controllen > 0 {
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
                while !cmsg.is_null() {
                    if (*cmsg).cmsg_level == libc::SOL_SOCKET
                        && (*cmsg).cmsg_type == libc::SCM_RIGHTS
                    {
                        let data_ptr = libc::CMSG_DATA(cmsg) as *const RawFd;
                        got_fd = Some(std::ptr::read(data_ptr));
                        break;
                    }
                    cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
                }
            }
        }
    }
    Ok(got_fd)
}
