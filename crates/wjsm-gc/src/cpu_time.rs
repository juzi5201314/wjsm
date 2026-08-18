//! 线程 CPU 时间助手。
//!
//! 归一化 GC 性能 gate 统一使用 thread CPU time（spec §18.4）；wall duration
//! 不得替代 CPU。不同 native OS 使用各自的线程 CPU 时钟实现。

/// 当前线程自启动以来的 CPU 累计纳秒；只用于 GC 工作前后的差分。
pub fn thread_cpu_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        linux_thread_cpu_ns()
    }
    #[cfg(target_os = "windows")]
    {
        windows_thread_cpu_ns()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
fn linux_thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` 是栈上有效指针；Linux 提供线程 CPU 时钟。
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    debug_assert_eq!(ret, 0, "CLOCK_THREAD_CPUTIME_ID 必须可用");
    if ret != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

#[cfg(target_os = "windows")]
fn windows_thread_cpu_ns() -> u64 {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

    let mut creation = windows_sys::Win32::Foundation::FILETIME::default();
    let mut exit = windows_sys::Win32::Foundation::FILETIME::default();
    let mut kernel = windows_sys::Win32::Foundation::FILETIME::default();
    let mut user = windows_sys::Win32::Foundation::FILETIME::default();
    // SAFETY: FILETIME 指针均指向有效的栈上可写缓冲区；伪句柄只在当前进程内有效。
    let ok = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return 0;
    }
    filetime_100ns(kernel)
        .saturating_add(filetime_100ns(user))
        .saturating_mul(100)
}

#[cfg(target_os = "windows")]
fn filetime_100ns(time: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
mod tests {
    use super::thread_cpu_ns;

    #[test]
    fn thread_cpu_ns_advances_with_burned_cpu() {
        let before = thread_cpu_ns();
        let mut sink: u64 = 0;
        for idx in 0..100_000u64 {
            sink = sink.wrapping_mul(3).wrapping_add(idx);
        }
        std::hint::black_box(sink);
        let after = thread_cpu_ns();
        assert!(after >= before, "thread CPU time 必须单调");
        assert!(after > before, "消耗 CPU 后计数必须前进");
    }
}
