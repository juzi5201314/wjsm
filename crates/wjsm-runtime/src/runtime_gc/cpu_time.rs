//! 线程 CPU 时间助手。
//!
//! 归一化 GC 性能 gate 统一使用 thread CPU time（spec §18.4）；
//! wall duration 不得替代 CPU：并发 worker 的 CPU 成本必须按各线程
//! CPU 时间求和，mutator 线程上的 pause/assist 同样按 CPU 时间计量。

/// 当前线程自启动以来的 CPU 累计纳秒（`CLOCK_THREAD_CPUTIME_ID`）。
///
/// 只用于 GC 工作前后的差分；绝对值无意义。
pub(crate) fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` 是栈上有效指针；Linux/WSL2 保证 CLOCK_THREAD_CPUTIME_ID 可用，
    // 失败只可能来自内核缺失，此时 debug 断言暴露，release 返回 0 差分。
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    debug_assert_eq!(ret, 0, "CLOCK_THREAD_CPUTIME_ID 必须可用");
    if ret != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

#[cfg(test)]
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
