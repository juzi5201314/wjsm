//! Inspector 共享可变状态：断点、步进、暂停、远程对象表。

use super::debug_info::DebugInfo;
use super::remote_object::RemoteObjectTable;
use std::collections::HashMap;
use tokio::sync::oneshot;

/// 主脚本固定 scriptId（CDP 字符串 id）。
pub(crate) const MAIN_SCRIPT_ID: &str = "1";

/// 断点查找键：(script_id, 0-based line)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BreakpointKey {
    pub script_id: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct BreakpointEntry {
    pub id: String,
    pub column: Option<u32>,
}

/// 步进模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum StepMode {
    #[default]
    None,
    Over,
    Into,
    Out,
}

/// 暂停原因（映射到 CDP `Debugger.paused.reason`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PauseReason {
    Other,
    DebugCommand,
    BreakOnStart,
    Step,
}

impl PauseReason {
    pub(crate) fn as_cdp(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::DebugCommand => "debugCommand",
            Self::BreakOnStart => "Break on start",
            Self::Step => "step",
        }
    }
}

/// resume 动作（由 CDP resume/step* 写入 oneshot）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumeAction {
    Continue,
    StepOver,
    StepInto,
    StepOut,
}

/// Inspector 内核状态（置于 `Mutex` 内）。
pub(crate) struct InspectorInner {
    pub debug_info: DebugInfo,
    pub breakpoints: HashMap<BreakpointKey, BreakpointEntry>,
    pub next_breakpoint_id: u64,
    pub step_mode: StepMode,
    pub paused: bool,
    pub break_on_start: bool,
    /// 是否已消费过 break_on_start。
    pub break_on_start_consumed: bool,
    pub debugger_enabled: bool,
    pub runtime_enabled: bool,
    pub last_pause_reason: Option<PauseReason>,
    pub resume_tx: Option<oneshot::Sender<ResumeAction>>,
    pub remote_objects: RemoteObjectTable,
    pub cached_call_frames: Vec<serde_json::Value>,
    /// 暂停时捕获的帧局部变量：`frame-N` → `[(展示名, NaN-box 原始值)]`。
    pub frame_locals: HashMap<String, Vec<(String, i64)>>,
    /// scriptParsed 是否已发送。
    pub script_parsed_sent: bool,
}

impl InspectorInner {
    pub(crate) fn new(debug_info: DebugInfo) -> Self {
        Self {
            debug_info,
            breakpoints: HashMap::new(),
            next_breakpoint_id: 1,
            step_mode: StepMode::None,
            paused: false,
            break_on_start: false,
            break_on_start_consumed: false,
            debugger_enabled: false,
            runtime_enabled: false,
            last_pause_reason: None,
            resume_tx: None,
            remote_objects: RemoteObjectTable::new(),
            cached_call_frames: Vec::new(),
            frame_locals: HashMap::new(),
            script_parsed_sent: false,
        }
    }

    /// 判定是否应在 `(line, col, flags)` 处暂停。
    ///
    /// `line`/`col` 为 **1-based** 源码坐标（与后端插桩约定一致）；内部断点表用 0-based CDP line。
    /// `flags & 1`：无条件 `debugger;` 语句。
    pub(crate) fn should_pause(
        &mut self,
        line: u32,
        col: u32,
        flags: i32,
    ) -> Option<(PauseReason, Vec<String>)> {
        let _ = col;
        // break_on_start：首次命中任意 debug_break 即停。
        if self.break_on_start && !self.break_on_start_consumed {
            self.break_on_start_consumed = true;
            return Some((PauseReason::BreakOnStart, Vec::new()));
        }

        if flags & 1 != 0 {
            return Some((PauseReason::DebugCommand, Vec::new()));
        }

        match self.step_mode {
            StepMode::None => {}
            StepMode::Over | StepMode::Into | StepMode::Out => {
                // 简化模型：任意步进模式下的下一次 debug_break 均暂停。
                self.step_mode = StepMode::None;
                return Some((PauseReason::Step, Vec::new()));
            }
        }

        // 1-based line → 0-based CDP lineNumber。
        let cdp_line = line.saturating_sub(1);
        let key = BreakpointKey {
            script_id: MAIN_SCRIPT_ID.to_string(),
            line: cdp_line,
        };
        if let Some(entry) = self.breakpoints.get(&key) {
            if let Some(bp_col) = entry.column {
                // 列级断点：col 亦为 1-based，CDP columnNumber 0-based。
                if col.saturating_sub(1) != bp_col {
                    return None;
                }
            }
            return Some((PauseReason::Other, vec![entry.id.clone()]));
        }

        None
    }

    pub(crate) fn set_breakpoint(
        &mut self,
        script_id: &str,
        line: u32,
        column: Option<u32>,
    ) -> String {
        let id = self.next_breakpoint_id.to_string();
        self.next_breakpoint_id += 1;
        self.breakpoints.insert(
            BreakpointKey {
                script_id: script_id.to_string(),
                line,
            },
            BreakpointEntry {
                id: id.clone(),
                column,
            },
        );
        id
    }

    pub(crate) fn remove_breakpoint(&mut self, breakpoint_id: &str) -> bool {
        let before = self.breakpoints.len();
        self.breakpoints
            .retain(|_, entry| entry.id != breakpoint_id);
        self.breakpoints.len() != before
    }
}
