use super::{HostResourceSnapshot, PlatformProbe};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
pub(super) fn snapshot(
    physical_total_bytes: u64,
    fallback_available_bytes: u64,
) -> HostResourceSnapshot {
    let mut probe = PlatformProbe::default();
    let values = match fs::read_to_string("/proc/meminfo") {
        Ok(input) => match parse_meminfo(&input) {
            Ok(values) => values,
            Err(error) => {
                probe
                    .errors
                    .push(format!("无法解析 /proc/meminfo：{error}"));
                Default::default()
            }
        },
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 /proc/meminfo：{error}"));
            Default::default()
        }
    };
    let available_bytes = values
        .get("MemAvailable")
        .copied()
        .unwrap_or(fallback_available_bytes);
    probe.swap_total_bytes = values.get("SwapTotal").copied();
    populate_cgroup_probe(&mut probe);
    probe.psi_full_avg10 = read_psi_full_avg10(Path::new("/proc/pressure/memory"), &mut probe);
    probe.address_space_limit_bytes = rlimit_as(&mut probe);
    HostResourceSnapshot::from_probe(physical_total_bytes, available_bytes, probe)
}

#[cfg(target_os = "linux")]
fn populate_cgroup_probe(probe: &mut PlatformProbe) {
    let Some(location) = current_cgroup_location(probe) else {
        return;
    };
    let path = location.path;
    probe.cgroup_path = Some(path.display().to_string());
    let memory_controller = path.join("memory.max").is_file();
    probe.cgroup_memory_controller = Some(memory_controller);
    if !memory_controller {
        return;
    }

    probe.cgroup_limit_bytes = read_limit(&path.join("memory.max"), probe);
    probe.cgroup_current_bytes = read_u64(&path.join("memory.current"), probe);
    let swap_max = read_limit(&path.join("memory.swap.max"), probe);
    let memory_events_available = path.join("memory.events").is_file();
    let writable = fs::metadata(path.join("memory.max"))
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false);
    let parent_enabled_memory = (location.relative != "/")
        .then(|| parent_has_memory_subtree_control(&path, probe))
        .unwrap_or(false);
    let delegated = location.relative != "/"
        && probe.cgroup_limit_bytes.is_some()
        && writable
        && parent_enabled_memory
        && swap_max == Some(0)
        && memory_events_available;
    probe.cgroup_delegated = Some(delegated);
    if delegated {
        probe.hard_isolation = true;
        probe.isolation_kind = Some("delegated-cgroup-v2".into());
    }
}

#[cfg(target_os = "linux")]
struct CgroupLocation {
    path: PathBuf,
    relative: String,
}

#[cfg(target_os = "linux")]
fn current_cgroup_location(probe: &mut PlatformProbe) -> Option<CgroupLocation> {
    let relative = current_cgroup_relative_path(probe)?;
    let (mount_root, mount_point) = cgroup2_mount(probe)?;
    let suffix = relative
        .strip_prefix(mount_root.trim_end_matches('/'))
        .unwrap_or(&relative)
        .trim_start_matches('/');
    Some(CgroupLocation {
        path: mount_point.join(suffix),
        relative,
    })
}

#[cfg(target_os = "linux")]
fn cgroup2_mount(probe: &mut PlatformProbe) -> Option<(String, PathBuf)> {
    let input = match fs::read_to_string("/proc/self/mountinfo") {
        Ok(input) => input,
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 /proc/self/mountinfo：{error}"));
            return None;
        }
    };
    for line in input.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        if after.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let fields: Vec<_> = before.split_whitespace().collect();
        let (Some(root), Some(mount_point)) = (fields.get(3), fields.get(4)) else {
            probe.errors.push("cgroup2 mountinfo 字段不完整".into());
            return None;
        };
        return Some((
            unescape_mount_path(root),
            PathBuf::from(unescape_mount_path(mount_point)),
        ));
    }
    probe.errors.push("未找到 cgroup2 mount".into());
    None
}

#[cfg(target_os = "linux")]
fn unescape_mount_path(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(target_os = "linux")]
fn parent_has_memory_subtree_control(path: &Path, probe: &mut PlatformProbe) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    match fs::read_to_string(parent.join("cgroup.subtree_control")) {
        Ok(controllers) => controllers.split_whitespace().any(|name| name == "memory"),
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 parent cgroup.subtree_control：{error}"));
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn current_cgroup_relative_path(probe: &mut PlatformProbe) -> Option<String> {
    let input = match fs::read_to_string("/proc/self/cgroup") {
        Ok(input) => input,
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 /proc/self/cgroup：{error}"));
            return None;
        }
    };
    input
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::to_owned)
        .or_else(|| {
            probe
                .errors
                .push("/proc/self/cgroup 未提供 cgroup v2 unified path".into());
            None
        })
}

#[cfg(target_os = "linux")]
fn parse_meminfo(input: &str) -> Result<std::collections::BTreeMap<String, u64>, String> {
    let mut values = std::collections::BTreeMap::new();
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let mut parts = value.split_whitespace();
        let Some(number) = parts.next() else {
            continue;
        };
        let multiplier = match parts.next().unwrap_or("B") {
            "kB" => 1024,
            "B" => 1,
            unit => return Err(format!("不支持的 meminfo 单位 `{unit}`")),
        };
        let value = number
            .parse::<u64>()
            .map_err(|error| format!("解析 {key} 失败：{error}"))?;
        values.insert(key.to_owned(), value.saturating_mul(multiplier));
    }
    Ok(values)
}

#[cfg(target_os = "linux")]
fn read_limit(path: &Path, probe: &mut PlatformProbe) -> Option<u64> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 {}：{error}", path.display()));
            return None;
        }
    };
    let value = value.trim();
    if value == "max" {
        return None;
    }
    match value.parse() {
        Ok(value) => Some(value),
        Err(error) => {
            probe
                .errors
                .push(format!("无法解析 {}：{error}", path.display()));
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn read_u64(path: &Path, probe: &mut PlatformProbe) -> Option<u64> {
    match fs::read_to_string(path) {
        Ok(value) => match value.trim().parse() {
            Ok(value) => Some(value),
            Err(error) => {
                probe
                    .errors
                    .push(format!("无法解析 {}：{error}", path.display()));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 {}：{error}", path.display()));
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn read_psi_full_avg10(path: &Path, probe: &mut PlatformProbe) -> Option<f64> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            probe
                .errors
                .push(format!("无法读取 {}：{error}", path.display()));
            return None;
        }
    };
    input
        .lines()
        .find(|line| line.starts_with("full "))
        .and_then(|line| {
            line.split_whitespace()
                .find(|part| part.starts_with("avg10="))
        })
        .and_then(|part| part.strip_prefix("avg10="))
        .and_then(|value| match value.parse() {
            Ok(value) => Some(value),
            Err(error) => {
                probe
                    .errors
                    .push(format!("无法解析 memory PSI full avg10：{error}"));
                None
            }
        })
}

#[cfg(target_os = "linux")]
fn rlimit_as(probe: &mut PlatformProbe) -> Option<u64> {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: `limit` 指向 libc `getrlimit` 的有效可写输出存储。
    let result = unsafe { libc::getrlimit(libc::RLIMIT_AS, limit.as_mut_ptr()) };
    if result != 0 {
        probe.errors.push(format!(
            "无法读取 RLIMIT_AS：{}",
            std::io::Error::last_os_error()
        ));
        return None;
    }
    // SAFETY: `getrlimit` 返回零保证完整初始化 `rlimit`。
    let limit = unsafe { limit.assume_init() };
    (limit.rlim_cur != libc::RLIM_INFINITY).then_some(limit.rlim_cur)
}

#[cfg(target_os = "windows")]
pub(super) fn snapshot(physical_total_bytes: u64, available_bytes: u64) -> HostResourceSnapshot {
    let mut probe = PlatformProbe::default();
    match windows_job_memory_limit() {
        Ok((limit, current, hard_isolation)) => {
            probe.job_limit_bytes = limit;
            probe.job_current_bytes = current;
            probe.hard_isolation = hard_isolation;
            probe.isolation_kind = hard_isolation.then(|| "windows-job-object".into());
        }
        Err(error) => probe.errors.push(error),
    }
    HostResourceSnapshot::from_probe(physical_total_bytes, available_bytes, probe)
}

#[cfg(target_os = "windows")]
fn windows_job_memory_limit() -> Result<(Option<u64>, Option<u64>, bool), String> {
    use std::mem::size_of;
    use windows_sys::Win32::System::JobObjects::{
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
    };

    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: null job handle 查询调用进程关联的 job；`info` 是有效输出 buffer。
    let success = unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut info).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("JOB_OBJECT structure size fits u32"),
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Ok((None, None, false));
    }
    let limited = info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0;
    Ok((
        limited.then_some(info.JobMemoryLimit as u64),
        limited.then_some(info.PeakJobMemoryUsed as u64),
        limited,
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn snapshot(physical_total_bytes: u64, available_bytes: u64) -> HostResourceSnapshot {
    HostResourceSnapshot::from_probe(
        physical_total_bytes,
        available_bytes,
        PlatformProbe::default(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(super) fn snapshot(physical_total_bytes: u64, available_bytes: u64) -> HostResourceSnapshot {
    HostResourceSnapshot::from_probe(
        physical_total_bytes,
        available_bytes,
        PlatformProbe::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn parse_meminfo_converts_kib_to_bytes() {
        let values = parse_meminfo("MemAvailable:       42 kB\nSwapTotal: 3 kB\n").unwrap();
        assert_eq!(values["MemAvailable"], 42 * 1024);
        assert_eq!(values["SwapTotal"], 3 * 1024);
    }
}
