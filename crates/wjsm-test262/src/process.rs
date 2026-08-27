use std::{
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_CAPTURE_LIMIT: usize = 1024 * 1024;
const PIPE_BUFFER_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl CapturedOutput {
    fn new(bytes: Vec<u8>, truncated: bool) -> Self {
        Self { bytes, truncated }
    }

    pub(crate) fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[output truncated after 1048576 bytes]");
        }
        text
    }
}

#[derive(Debug)]
pub(crate) struct CompletedProcess {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: CapturedOutput,
    pub(crate) stderr: CapturedOutput,
}

#[derive(Debug)]
pub(crate) enum ProcessOutcome {
    Completed(CompletedProcess),
    TimedOut {
        stdout: CapturedOutput,
        stderr: CapturedOutput,
    },
}

pub(crate) fn run_with_input(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
) -> Result<ProcessOutcome, String> {
    spawn_in_own_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn wjsm: {error}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to open child stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to open child stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to open child stderr".to_string())?;

    let stdin_writer = thread::spawn(move || write_child_input(stdin, input));
    let stdout_reader = thread::spawn(move || read_capped(stdout));
    let stderr_reader = thread::spawn(move || read_capped(stderr));

    let wait_result = wait_child_timeout(&mut child, timeout)
        .map_err(|error| format!("failed to wait for wjsm: {error}"))?;
    let timed_out = wait_result.is_none();

    if timed_out {
        kill_child_process_tree(&mut child);
        let _ = child.wait();
    }

    let write_result = join_thread(stdin_writer, "stdin writer")?;
    let stdout = join_io_thread(stdout_reader, "stdout reader")?;
    let stderr = join_io_thread(stderr_reader, "stderr reader")?;

    if timed_out {
        return Ok(ProcessOutcome::TimedOut { stdout, stderr });
    }

    write_result.map_err(|error| format!("failed to write to stdin: {error}"))?;

    Ok(ProcessOutcome::Completed(CompletedProcess {
        status: wait_result.expect("checked not timed out"),
        stdout,
        stderr,
    }))
}

/// 让子进程成为新进程组组长，超时时可整组回收。
///
/// 读线程要等 stdout/stderr 管道全部写端关闭才能返回；若被 kill 的直接子进程
/// 又 fork 过持有管道写端的孙进程（例如某些 `sh -c` 实现不做隐式 exec 优化），
/// 只杀直接子进程会让读线程阻塞到孙进程自然退出。
#[cfg(unix)]
fn spawn_in_own_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn spawn_in_own_process_group(_command: &mut Command) {}

/// 超时后回收整棵子进程树。
#[cfg(unix)]
fn kill_child_process_tree(child: &mut Child) {
    // 直接子进程即组长且尚未被 wait() 回收，pid 不会被复用，
    // 负 pgid 只命中该进程组，不会误杀无关进程。
    let pgid = child.id() as libc::pid_t;
    // SAFETY: kill 是 async-signal-safe 系统调用；组不存在时返回 ESRCH，忽略即可。
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_child_process_tree(child: &mut Child) {
    let _ = child.kill();
}

fn wait_child_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    if timeout.is_zero() {
        return child.wait().map(Some);
    }

    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None if start.elapsed() >= timeout => return Ok(None),
            None => thread::sleep(CHILD_POLL_INTERVAL),
        }
    }
}

fn write_child_input(mut stdin: impl Write, input: Vec<u8>) -> io::Result<()> {
    stdin.write_all(&input)
}

fn read_capped(mut reader: impl Read) -> io::Result<CapturedOutput> {
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; PIPE_BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let remaining = OUTPUT_CAPTURE_LIMIT.saturating_sub(captured.len());
        if remaining >= read {
            captured.extend_from_slice(&buffer[..read]);
        } else {
            captured.extend_from_slice(&buffer[..remaining]);
            truncated = true;
        }
    }

    Ok(CapturedOutput::new(captured, truncated))
}

fn join_thread<T>(handle: thread::JoinHandle<T>, name: &str) -> Result<T, String> {
    handle.join().map_err(|_| format!("{name} panicked"))
}

fn join_io_thread<T>(handle: thread::JoinHandle<io::Result<T>>, name: &str) -> Result<T, String> {
    join_thread(handle, name)?.map_err(|error| format!("{name} failed: {error}"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{process::Stdio, time::Duration};

    use super::*;

    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    #[test]
    fn run_with_input_collects_stdout_and_stderr() {
        let outcome = run_with_input(
            shell_command("cat; printf err >&2"),
            b"hello".to_vec(),
            Duration::from_secs(1),
        )
        .expect("process output");

        let ProcessOutcome::Completed(output) = outcome else {
            panic!("process should complete");
        };
        assert!(output.status.success());
        assert_eq!(output.stdout.text(), "hello");
        assert_eq!(output.stderr.text(), "err");
    }

    #[test]
    fn run_with_input_times_out_hanging_child() {
        let start = Instant::now();
        let outcome = run_with_input(
            shell_command("sleep 2"),
            Vec::new(),
            Duration::from_millis(50),
        )
        .expect("timeout outcome");

        assert!(matches!(outcome, ProcessOutcome::TimedOut { .. }));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn run_with_input_timeout_reaps_grandchild_holding_pipes() {
        // 回归：后台孙进程继承 stdout 写端。超时必须整组回收，
        // 否则读线程会阻塞到孙进程自然退出，且捕获到超时后才产生的输出。
        let start = Instant::now();
        let outcome = run_with_input(
            shell_command("(sleep 2; echo late) & sleep 2"),
            Vec::new(),
            Duration::from_millis(50),
        )
        .expect("timeout outcome");

        let ProcessOutcome::TimedOut { stdout, .. } = outcome else {
            panic!("process should time out");
        };
        assert!(
            !stdout.text().contains("late"),
            "grandchild must be killed before it writes: {:?}",
            stdout.text()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
