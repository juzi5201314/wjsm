//! Inspector 共享可变状态：断点、步进、暂停、远程对象表、暂停期命令通道。

use super::debug_info::DebugInfo;
use super::remote_object::RemoteObjectTable;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

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

/// 步进目标：相对暂停点的深度/位置约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StepTarget {
    pub mode: StepMode,
    /// 发起 step 时的调用栈深度（0 = 最内层）。
    pub start_depth: u32,
    /// 发起 step 时的 1-based 源码位置。
    pub start_line: u32,
    pub start_col: u32,
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

/// 暂停期间需要 `Caller`/Store 的 CDP 命令。
pub(crate) enum PauseCommand {
    GetProperties {
        object_id: String,
        reply: oneshot::Sender<serde_json::Value>,
    },
    EvaluateOnFrame {
        frame_id: String,
        expression: String,
        reply: oneshot::Sender<serde_json::Value>,
    },
    EvaluateGlobal {
        expression: String,
        reply: oneshot::Sender<serde_json::Value>,
    },
}

/// Inspector 内核状态（置于 `Mutex` 内）。
pub(crate) struct InspectorInner {
    pub debug_info: DebugInfo,
    pub breakpoints: HashMap<BreakpointKey, BreakpointEntry>,
    pub next_breakpoint_id: u64,
    pub step_mode: StepMode,
    /// 精细步进目标（depth/line）；`None` 表示未处于 step。
    pub step_target: Option<StepTarget>,
    pub paused: bool,
    pub break_on_start: bool,
    /// 是否已消费过 break_on_start。
    pub break_on_start_consumed: bool,
    pub debugger_enabled: bool,
    pub runtime_enabled: bool,
    pub last_pause_reason: Option<PauseReason>,
    pub resume_tx: Option<oneshot::Sender<ResumeAction>>,
    /// 当前暂停会话的命令发送端（仅 paused 时有效）。
    pub pause_cmd_tx: Option<mpsc::UnboundedSender<PauseCommand>>,
    pub remote_objects: RemoteObjectTable,
    pub cached_call_frames: Vec<serde_json::Value>,
    /// 暂停时捕获的帧局部变量：`frame-N` → `[(展示名, NaN-box 原始值)]`。
    pub frame_locals: HashMap<String, Vec<(String, i64)>>,
    /// 当前暂停点 1-based 位置与栈深。
    pub pause_line: u32,
    pub pause_col: u32,
    pub pause_depth: u32,
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
            step_target: None,
            paused: false,
            break_on_start: false,
            break_on_start_consumed: false,
            debugger_enabled: false,
            runtime_enabled: false,
            last_pause_reason: None,
            resume_tx: None,
            pause_cmd_tx: None,
            remote_objects: RemoteObjectTable::new(),
            cached_call_frames: Vec::new(),
            frame_locals: HashMap::new(),
            pause_line: 1,
            pause_col: 1,
            pause_depth: 0,
            script_parsed_sent: false,
        }
    }

    /// 判定是否应在 `(line, col, flags, frame_depth)` 处暂停。
    ///
    /// `line`/`col` 为 **1-based**；`frame_depth` 为 0=最内层。
    /// `flags & 1`：无条件 `debugger;` 语句。
    pub(crate) fn should_pause(
        &mut self,
        line: u32,
        col: u32,
        flags: i32,
        frame_depth: u32,
    ) -> Option<(PauseReason, Vec<String>)> {
        // break_on_start：首次命中任意 debug_break 即停。
        if self.break_on_start && !self.break_on_start_consumed {
            self.break_on_start_consumed = true;
            return Some((PauseReason::BreakOnStart, Vec::new()));
        }

        // debugger; 无条件暂停（步进中也尊重）。
        if flags & 1 != 0 {
            self.step_target = None;
            self.step_mode = StepMode::None;
            return Some((PauseReason::DebugCommand, Vec::new()));
        }

        if let Some(step) = self.step_target {
            let location_changed = line != step.start_line || col != step.start_col;
            let hit = match step.mode {
                StepMode::None => false,
                // step over：同一或更浅深度，且离开原语句位置。
                StepMode::Over => frame_depth <= step.start_depth && location_changed,
                // step into：进入更深，或同层换行，或任意新位置。
                StepMode::Into => {
                    frame_depth > step.start_depth
                        || (frame_depth == step.start_depth && location_changed)
                }
                // step out：返回到更浅帧。
                StepMode::Out => frame_depth < step.start_depth,
            };
            if hit {
                self.step_target = None;
                self.step_mode = StepMode::None;
                return Some((PauseReason::Step, Vec::new()));
            }
            // 步进中不命中用户断点以外的逻辑时继续；仍检查用户断点。
        }

        // 1-based line → 0-based CDP lineNumber。
        let cdp_line = line.saturating_sub(1);
        let key = BreakpointKey {
            script_id: MAIN_SCRIPT_ID.to_string(),
            line: cdp_line,
        };
        if let Some(entry) = self.breakpoints.get(&key) {
            if let Some(bp_col) = entry.column {
                if col.saturating_sub(1) != bp_col {
                    return None;
                }
            }
            // 命中断点时清除步进状态。
            self.step_target = None;
            self.step_mode = StepMode::None;
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

    /// 根据 resume 动作安装步进目标。
    pub(crate) fn apply_resume_action(&mut self, action: ResumeAction) {
        match action {
            ResumeAction::Continue => {
                self.step_mode = StepMode::None;
                self.step_target = None;
            }
            ResumeAction::StepOver => {
                self.step_mode = StepMode::Over;
                self.step_target = Some(StepTarget {
                    mode: StepMode::Over,
                    start_depth: self.pause_depth,
                    start_line: self.pause_line,
                    start_col: self.pause_col,
                });
            }
            ResumeAction::StepInto => {
                self.step_mode = StepMode::Into;
                self.step_target = Some(StepTarget {
                    mode: StepMode::Into,
                    start_depth: self.pause_depth,
                    start_line: self.pause_line,
                    start_col: self.pause_col,
                });
            }
            ResumeAction::StepOut => {
                self.step_mode = StepMode::Out;
                self.step_target = Some(StepTarget {
                    mode: StepMode::Out,
                    start_depth: self.pause_depth,
                    start_line: self.pause_line,
                    start_col: self.pause_col,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner() -> InspectorInner {
        let mut i = InspectorInner::new(DebugInfo::default());
        i.pause_line = 10;
        i.pause_col = 1;
        i.pause_depth = 1;
        i
    }

    #[test]
    fn step_over_waits_for_same_or_shallower_depth_new_line() {
        let mut i = inner();
        i.apply_resume_action(ResumeAction::StepOver);
        // 更深：不暂停
        assert!(i.should_pause(11, 1, 0, 2).is_none());
        // 同深度同行：不暂停
        assert!(i.should_pause(10, 1, 0, 1).is_none());
        // 同深度新行：暂停
        let hit = i.should_pause(11, 1, 0, 1);
        assert!(matches!(hit, Some((PauseReason::Step, _))));
    }

    #[test]
    fn step_out_requires_shallower_depth() {
        let mut i = inner();
        i.apply_resume_action(ResumeAction::StepOut);
        assert!(i.should_pause(20, 1, 0, 1).is_none());
        assert!(i.should_pause(20, 1, 0, 2).is_none());
        let hit = i.should_pause(20, 1, 0, 0);
        assert!(matches!(hit, Some((PauseReason::Step, _))));
    }

    #[test]
    fn step_into_stops_on_deeper_frame() {
        let mut i = inner();
        i.apply_resume_action(ResumeAction::StepInto);
        let hit = i.should_pause(10, 1, 0, 2);
        assert!(matches!(hit, Some((PauseReason::Step, _))));
    }

    #[test]
    fn debugger_flag_always_pauses() {
        let mut i = inner();
        i.apply_resume_action(ResumeAction::StepOver);
        let hit = i.should_pause(10, 1, 1, 5);
        assert!(matches!(hit, Some((PauseReason::DebugCommand, _))));
    }
}
