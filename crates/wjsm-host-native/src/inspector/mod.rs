//! Native runtime 的 Chrome DevTools Protocol inspector owner。

mod remote;
mod server;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use serde_json::{Value, json};
use wjsm_artifact_format::PortableArtifact;
use wjsm_native_abi::NativeVmContext;

use crate::NativeAgentState;

pub use server::InspectorConfig;
use server::{InspectorCommand, InspectorServer, ScriptInfo, ServerEvent};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PauseOnExceptions {
    None,
    Uncaught,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepMode {
    Into,
    Over(u32),
    Out(u32),
}

#[derive(Clone, Debug)]
struct Breakpoint {
    url: Option<String>,
    line: u32,
    column: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandOutcome {
    StayPaused,
    Resume,
}

pub(crate) struct InspectorRuntime {
    server: InspectorServer,
    commands: Receiver<InspectorCommand>,
    events: Sender<ServerEvent>,
    break_on_start: bool,
    first_check: bool,
    pause_requested: bool,
    pause_on_exceptions: PauseOnExceptions,
    step: Option<StepMode>,
    breakpoints: HashMap<String, Breakpoint>,
    next_breakpoint: u64,
    script: Option<ScriptInfo>,
    current_line: u32,
    current_column: u32,
    current_function: u32,
    evaluating: bool,
}

impl InspectorRuntime {
    pub(crate) fn start(config: InspectorConfig) -> std::io::Result<Self> {
        let break_on_start = config.break_on_start;
        let (server, commands, events) = InspectorServer::start(config)?;
        Ok(Self {
            server,
            commands,
            events,
            break_on_start,
            first_check: true,
            pause_requested: false,
            pause_on_exceptions: PauseOnExceptions::None,
            step: None,
            breakpoints: HashMap::new(),
            next_breakpoint: 1,
            script: None,
            current_line: 1,
            current_column: 1,
            current_function: 0,
            evaluating: false,
        })
    }

    pub(crate) fn url(&self) -> &str {
        self.server.url()
    }

    pub(crate) fn register_script(&mut self, artifact: &PortableArtifact) {
        let url = artifact
            .manifest()
            .modules
            .iter()
            .find(|module| module.id == artifact.manifest().entry)
            .or_else(|| artifact.manifest().modules.first())
            .map(|module| module.logical_url.clone())
            .unwrap_or_else(|| "input.js".into());
        let script = ScriptInfo {
            id: "1".into(),
            url,
            source: artifact.source_text().unwrap_or_default().to_owned(),
            hash: hex_digest(artifact.digest()),
        };
        self.script = Some(script.clone());
        let _ = self.events.send(ServerEvent::Script(script));
    }

    fn check(
        &mut self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        function: u32,
        line: u32,
        column: u32,
    ) {
        if self.evaluating {
            return;
        }
        self.current_function = function;
        self.current_line = line;
        self.current_column = column;
        let mut resume = false;
        loop {
            match self.commands.try_recv() {
                Ok(command) => {
                    if self.handle_command(ctx, state, command) == CommandOutcome::Resume {
                        resume = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let should_pause = !resume
            && (self.pause_requested
                || self.first_check && self.break_on_start
                || self.matches_breakpoint(line, column)
                || self.matches_step(ctx));
        self.first_check = false;
        if should_pause {
            self.pause_requested = false;
            self.step = None;
            self.pause_loop(ctx, state, "other");
        }
    }

    fn pause_for_exception(
        &mut self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        exception: i64,
        uncaught: bool,
    ) {
        let should_pause = match self.pause_on_exceptions {
            PauseOnExceptions::None => false,
            PauseOnExceptions::Uncaught => uncaught,
            PauseOnExceptions::All => !uncaught,
        };
        if !self.evaluating && should_pause {
            let data = remote::remote_object(state, exception);
            self.send_event(json!({
                "method": "Debugger.paused",
                "params": {
                    "reason": "exception",
                    "data": data,
                    "hitBreakpoints": [],
                    "callFrames": [self.call_frame(state)],
                }
            }));
            self.wait_until_resumed(ctx, state);
        }
    }

    fn pause_loop(
        &mut self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        reason: &str,
    ) {
        let hit_breakpoints = self
            .matching_breakpoint_ids(self.current_line, self.current_column)
            .collect::<Vec<_>>();
        self.send_event(json!({
            "method": "Debugger.paused",
            "params": {
                "reason": reason,
                "hitBreakpoints": hit_breakpoints,
                "callFrames": [self.call_frame(state)],
            }
        }));
        self.wait_until_resumed(ctx, state);
    }

    fn wait_until_resumed(&mut self, ctx: &mut NativeVmContext, state: &mut NativeAgentState) {
        while let Ok(command) = self.commands.recv() {
            if self.handle_command(ctx, state, command) == CommandOutcome::Resume {
                self.send_event(json!({"method": "Debugger.resumed", "params": {}}));
                break;
            }
        }
    }

    fn handle_command(
        &mut self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        command: InspectorCommand,
    ) -> CommandOutcome {
        let InspectorCommand { id, method, params } = command;
        match method.as_str() {
            "Runtime.runIfWaitingForDebugger" | "Debugger.resume" => {
                self.send_result(id, json!({}));
                CommandOutcome::Resume
            }
            "Debugger.stepInto" => {
                self.step = Some(StepMode::Into);
                self.send_result(id, json!({}));
                CommandOutcome::Resume
            }
            "Debugger.stepOver" => {
                self.step = Some(StepMode::Over(ctx.js_call_depth));
                self.send_result(id, json!({}));
                CommandOutcome::Resume
            }
            "Debugger.stepOut" => {
                self.step = Some(StepMode::Out(ctx.js_call_depth));
                self.send_result(id, json!({}));
                CommandOutcome::Resume
            }
            "Debugger.pause" => {
                self.pause_requested = true;
                self.send_result(id, json!({}));
                CommandOutcome::StayPaused
            }
            "Debugger.setPauseOnExceptions" => {
                let state_name = params
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                self.pause_on_exceptions = match state_name {
                    "all" => PauseOnExceptions::All,
                    "uncaught" => PauseOnExceptions::Uncaught,
                    _ => PauseOnExceptions::None,
                };
                self.send_result(id, json!({}));
                CommandOutcome::StayPaused
            }
            "Debugger.setBreakpointByUrl" => {
                self.set_breakpoint(id, &params);
                CommandOutcome::StayPaused
            }
            "Debugger.removeBreakpoint" => {
                if let Some(breakpoint_id) = params.get("breakpointId").and_then(Value::as_str) {
                    self.breakpoints.remove(breakpoint_id);
                }
                self.send_result(id, json!({}));
                CommandOutcome::StayPaused
            }
            "Runtime.evaluate" | "Debugger.evaluateOnCallFrame" => {
                self.evaluate(ctx, state, id, &params);
                CommandOutcome::StayPaused
            }
            "Runtime.getProperties" => {
                let result = params
                    .get("objectId")
                    .and_then(Value::as_str)
                    .and_then(remote::decode_object_id)
                    .map(|object| remote::properties(state, object))
                    .unwrap_or_default();
                self.send_result(id, json!({"result": result, "internalProperties": []}));
                CommandOutcome::StayPaused
            }
            _ => {
                self.send_error(id, -32601, format!("unsupported CDP method {method}"));
                CommandOutcome::StayPaused
            }
        }
    }

    fn set_breakpoint(&mut self, id: Value, params: &Value) {
        let line = params
            .get("lineNumber")
            .and_then(Value::as_u64)
            .and_then(|line| u32::try_from(line).ok())
            .unwrap_or(0)
            .saturating_add(1);
        let column = params
            .get("columnNumber")
            .and_then(Value::as_u64)
            .and_then(|column| u32::try_from(column).ok())
            .unwrap_or(0)
            .saturating_add(1);
        let breakpoint_id = format!("wjsm:{}:{}", line - 1, self.next_breakpoint);
        self.next_breakpoint += 1;
        self.breakpoints.insert(
            breakpoint_id.clone(),
            Breakpoint {
                url: params.get("url").and_then(Value::as_str).map(str::to_owned),
                line,
                column,
            },
        );
        self.send_result(
            id,
            json!({
                "breakpointId": breakpoint_id,
                "locations": [{
                    "scriptId": self.script_id(),
                    "lineNumber": line - 1,
                    "columnNumber": column - 1,
                }],
            }),
        );
    }

    fn evaluate(
        &mut self,
        ctx: &mut NativeVmContext,
        state: &mut NativeAgentState,
        id: Value,
        params: &Value,
    ) {
        let Some(expression) = params.get("expression").and_then(Value::as_str) else {
            self.send_error(id, -32602, "Runtime.evaluate requires expression".into());
            return;
        };
        let Some(global) = state.global_object else {
            self.send_error(id, -32000, "global realm is not initialized".into());
            return;
        };
        self.evaluating = true;
        let result = crate::dispatch::modules::execute_vm_script(
            ctx,
            state,
            expression,
            global,
            "inspector:evaluate",
        );
        self.evaluating = false;
        match result {
            Ok(result) => {
                self.send_result(id, json!({"result": remote::remote_object(state, result)}))
            }
            Err(error) => self.send_result(
                id,
                json!({
                    "result": {"type": "undefined"},
                    "exceptionDetails": {
                        "text": error.to_string(),
                        "lineNumber": self.current_line.saturating_sub(1),
                        "columnNumber": self.current_column.saturating_sub(1),
                    }
                }),
            ),
        }
    }

    fn matches_breakpoint(&self, line: u32, column: u32) -> bool {
        self.matching_breakpoint_ids(line, column).next().is_some()
    }

    fn matching_breakpoint_ids(&self, line: u32, column: u32) -> impl Iterator<Item = String> + '_ {
        self.breakpoints.iter().filter_map(move |(id, breakpoint)| {
            let url_matches = breakpoint.url.as_ref().is_none_or(|expected| {
                self.script
                    .as_ref()
                    .is_some_and(|script| script.url == *expected)
            });
            (url_matches && breakpoint.line == line && breakpoint.column <= column)
                .then(|| id.clone())
        })
    }

    fn matches_step(&self, ctx: &NativeVmContext) -> bool {
        match self.step {
            None => false,
            Some(StepMode::Into) => true,
            Some(StepMode::Over(depth)) => ctx.js_call_depth <= depth,
            Some(StepMode::Out(depth)) => ctx.js_call_depth < depth,
        }
    }

    fn call_frame(&self, state: &NativeAgentState) -> Value {
        let function_name = state
            .function_names
            .get(self.current_function as usize)
            .cloned()
            .unwrap_or_else(|| "(program)".into());
        json!({
            "callFrameId": "0",
            "functionName": function_name,
            "functionLocation": {
                "scriptId": self.script_id(),
                "lineNumber": self.current_line.saturating_sub(1),
                "columnNumber": self.current_column.saturating_sub(1),
            },
            "location": {
                "scriptId": self.script_id(),
                "lineNumber": self.current_line.saturating_sub(1),
                "columnNumber": self.current_column.saturating_sub(1),
            },
            "url": self.script.as_ref().map_or("", |script| script.url.as_str()),
            "scopeChain": [{
                "type": "global",
                "name": "Global",
                "object": state.global_object.map_or_else(
                    || json!({"type": "undefined"}),
                    |global| remote::remote_object(state, global),
                ),
            }],
            "this": state.global_object.map_or_else(
                || json!({"type": "undefined"}),
                |global| remote::remote_object(state, global),
            ),
        })
    }

    fn script_id(&self) -> &str {
        self.script
            .as_ref()
            .map_or("1", |script| script.id.as_str())
    }

    fn send_result(&self, id: Value, result: Value) {
        self.send_event(json!({"id": id, "result": result}));
    }

    fn send_error(&self, id: Value, code: i64, message: String) {
        self.send_event(json!({"id": id, "error": {"code": code, "message": message}}));
    }

    fn send_event(&self, message: Value) {
        let _ = self.events.send(ServerEvent::Protocol(message));
    }
}

pub(crate) fn debug_check(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    function: u32,
    line: u32,
    column: u32,
) {
    let Some(mut inspector) = state.inspector.take() else {
        return;
    };
    inspector.check(ctx, state, function, line, column);
    state.inspector = Some(inspector);
}
pub(crate) fn pause(ctx: &mut NativeVmContext, state: &mut NativeAgentState, reason: &str) {
    let Some(mut inspector) = state.inspector.take() else {
        return;
    };
    inspector.pause_loop(ctx, state, reason);
    state.inspector = Some(inspector);
}

pub(crate) fn pause_for_exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    exception: i64,
    uncaught: bool,
) {
    let Some(mut inspector) = state.inspector.take() else {
        return;
    };
    inspector.pause_for_exception(ctx, state, exception, uncaught);
    state.inspector = Some(inspector);
}

pub(crate) fn poll(ctx: &mut NativeVmContext, state: &mut NativeAgentState) {
    let function = state
        .inspector
        .as_ref()
        .map_or(0, |inspector| inspector.current_function);
    let line = state
        .inspector
        .as_ref()
        .map_or(1, |inspector| inspector.current_line);
    let column = state
        .inspector
        .as_ref()
        .map_or(1, |inspector| inspector.current_column);
    debug_check(ctx, state, function, line, column);
}

fn hex_digest(digest: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
