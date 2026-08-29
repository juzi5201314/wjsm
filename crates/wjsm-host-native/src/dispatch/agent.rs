//! test262 `$262.agent` harness 的 native owner。
//!
//! 语义以 V8 `test/test262/harness-agent.js` 为基准：
//!
//! - `$262.agent.start(script)`：启动一个独立 agent 线程执行 `script`；
//!   agent 拥有自己的 runtime/heap，但与主 agent 共享 cluster
//!   （SAB backing、广播、报告）。
//! - `$262.agent.broadcast(sab, value)`：把 SAB（连同可选数值）广播给所有
//!   存活 agent，并**阻塞**至所有 agent 已 retrieve（V8 用共享内存计数，
//!   这里用 cluster 级确认表 + condvar）。
//! - `$262.agent.receiveBroadcast(callback)`：agent 内注册广播回调；广播
//!   到达时以 `(sab, value)` 调用。
//! - `$262.agent.report(msg)`：agent 把字符串压入 cluster 报告队列。
//! - `$262.agent.getReport()`：main 拉取队首报告；无则 null。
//! - `$262.agent.sleep(ms)` / `$262.agent.monotonicNow()`：时间工具。
//! - `$262.agent.leaving()`：test262 提示 agent 即将退出的 no-op。

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::node_worker_threads::{AgentCommand, Test262AgentContext};
use super::runtime::{fail_dispatch, render_value, to_number};
use crate::{NativeAgentState, NativeCallableKind, NativeRuntime};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Test262Method {
    Start,
    Broadcast,
    ReceiveBroadcast,
    GetReport,
    Report,
    Sleep,
    MonotonicNow,
    Leaving,
    CreateRealm,
}

/// agent 线程内的 `$262.agent` 状态。
pub(crate) struct Test262AgentState {
    /// receiveBroadcast 注册的回调（JS callable handle）。
    pub(crate) receive_callback: Option<i64>,
    /// 缓存 `$262` 对象 handle。
    pub(crate) bridge: Option<i64>,
}

impl Test262AgentState {
    pub(crate) fn new() -> Self {
        Self {
            receive_callback: None,
            bridge: None,
        }
    }
}

/// 主 agent 侧 `$262.agent`：构造 `$262` 对象（含 `agent` 子对象）。
///
/// agent 线程与主 agent 使用同一方法表；`start/broadcast/getReport` 只在
/// 主 agent 语义有效，`receiveBroadcast/report` 在 agent 线程语义有效，
/// `sleep/monotonicNow/leaving` 两者皆可。
pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    let cached = if let Some(bridge) = state.test262_agent.as_ref().and_then(|agent| agent.bridge) {
        bridge
    } else if let Some(bridge) = state.agent_bridge {
        bridge
    } else {
        let agent = state.allocate_object(8, false).ok()?;
        for (name, method) in [
            ("start", Test262Method::Start),
            ("broadcast", Test262Method::Broadcast),
            ("receiveBroadcast", Test262Method::ReceiveBroadcast),
            ("getReport", Test262Method::GetReport),
            ("report", Test262Method::Report),
            ("sleep", Test262Method::Sleep),
            ("monotonicNow", Test262Method::MonotonicNow),
            ("leaving", Test262Method::Leaving),
        ] {
            let key = state.intern_property_string(name.into())?;
            let callable = state.native_callable(NativeCallableKind::Test262Agent(method))?;
            state
                .gc
                .heap()
                .set_property(value::decode_handle(agent), key, callable as u64)
                .ok()?;
        }
        let bridge = state.allocate_object(3, false).ok()?;
        let agent_key = state.intern_property_string("agent".into())?;
        state
            .gc
            .heap()
            .set_property(value::decode_handle(bridge), agent_key, agent as u64)
            .ok()?;
        let create_realm =
            state.native_callable(NativeCallableKind::Test262Agent(Test262Method::CreateRealm))?;
        let create_key = state.intern_property_string("createRealm".into())?;
        state
            .gc
            .heap()
            .set_property(
                value::decode_handle(bridge),
                create_key,
                create_realm as u64,
            )
            .ok()?;
        if let Some(gc) = state.native_callable(NativeCallableKind::Gc) {
            let gc_key = state.intern_property_string("gc".into())?;
            let _ = state
                .gc
                .heap()
                .set_property(value::decode_handle(bridge), gc_key, gc as u64);
        }
        if let Some(test262) = state.test262_agent.as_mut() {
            test262.bridge = Some(bridge);
        } else {
            state.agent_bridge = Some(bridge);
        }
        bridge
    };
    Some(cached)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: Test262Method,
    args: &[i64],
) -> i64 {
    match method {
        Test262Method::Start => start(ctx, state, args),
        Test262Method::Broadcast => broadcast(ctx, state, args),
        Test262Method::ReceiveBroadcast => receive_broadcast(ctx, state, args),
        Test262Method::GetReport => get_report(ctx, state),
        Test262Method::Report => report(state, args),
        Test262Method::Sleep => sleep(ctx, state, args),
        Test262Method::MonotonicNow => monotonic_now(state),
        Test262Method::Leaving => value::encode_undefined(),
        Test262Method::CreateRealm => create_realm(ctx, state),
    }
}

/// `$262.createRealm()`：返回 `{ global }`。当前与主 realm 共享同一全局（同 agent）。
fn create_realm(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let Some(global) = state.global_object else {
        return fail_dispatch(ctx);
    };
    let Ok(record) = state.allocate_object_with_gc_retry(ctx, 1, false) else {
        return fail_dispatch(ctx);
    };
    let Some(key) = state.intern_property_string("global".into()) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_property(value::decode_handle(record), key, global as u64)
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    record
}

/// `$262.agent.start(script)`：编译 script 并在独立线程执行。
fn start(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(script) = args
        .first()
        .and_then(|encoded| state.string_owned(*encoded))
        .and_then(|text| text.to_utf8())
    else {
        return type_error(ctx, state, "start: script must be a string");
    };
    let (agent_id, command_rx) = state.node_worker_threads.cluster.agent.register_agent();
    let context = Test262AgentContext {
        cluster: Arc::clone(&state.node_worker_threads.cluster),
        agent_id,
        command_rx,
        runtime_config: state.runtime_config.child_config(),
    };
    let root = state.working_directory.clone();
    let spawn = std::thread::Builder::new()
        .name(format!("wjsm-test262-agent-{agent_id}"))
        .spawn(move || run_agent(script, root, context));
    match spawn {
        Ok(_) => value::encode_f64(f64::from(agent_id)),
        Err(error) => {
            state
                .node_worker_threads
                .cluster
                .agent
                .unregister_agent(agent_id);
            type_error(ctx, state, &format!("start: agent spawn failed: {error}"))
        }
    }
}

/// `$262.agent.broadcast(sab, value)`：广播并阻塞至所有存活 agent retrieve。
fn broadcast(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(sab) = args.first().copied() else {
        return type_error(ctx, state, "broadcast: SharedArrayBuffer expected");
    };
    let handle = value::decode_handle(sab);
    let Some(entry) = state
        .shared_array_buffers
        .get(&handle)
        .map(|entry| entry.backing_id)
    else {
        return type_error(ctx, state, "broadcast: sab must be a SharedArrayBuffer");
    };
    let value_arg = args
        .get(1)
        .copied()
        .filter(|encoded| !value::is_undefined(*encoded));
    let seq = state
        .node_worker_threads
        .cluster
        .agent
        .register_broadcast(entry, value_arg);
    let cluster = state.node_worker_threads.cluster.clone();
    let targets = cluster.agent.agent_ids();
    let target = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    for agent_id in targets {
        let delivered = cluster
            .agent
            .send_command(agent_id, AgentCommand::Broadcast(seq));
        if !delivered {
            // agent 已消失：视作已 retrieve，避免永久阻塞。
            cluster.agent.confirm_broadcast(seq);
        }
    }
    if target > 0 {
        cluster.agent.wait_broadcast(seq, target);
    }
    value::encode_undefined()
}

/// `$262.agent.receiveBroadcast(callback)`：注册广播回调。
fn receive_broadcast(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(callback) = args.first().copied() else {
        return type_error(ctx, state, "receiveBroadcast: callback expected");
    };
    if !value::is_callable(callback) {
        return type_error(ctx, state, "receiveBroadcast: callback must be callable");
    }
    let Some(test262) = state.test262_agent.as_mut() else {
        return fail_dispatch(ctx);
    };
    test262.receive_callback = Some(callback);
    value::encode_undefined()
}

/// `$262.agent.getReport()`：取队首报告；无则 null。
fn get_report(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let report = state.node_worker_threads.cluster.agent.pop_report();
    report.map_or_else(value::encode_null, |text| {
        state
            .intern_text(text, value::TAG_STRING)
            .unwrap_or_else(|| fail_dispatch(ctx))
    })
}

/// `$262.agent.report(msg)`：把消息压入报告队列。
fn report(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let message = args.first().copied().map_or_else(
        || "undefined".to_string(),
        |encoded| render_value(state, encoded),
    );
    state.node_worker_threads.cluster.agent.push_report(message);
    value::encode_undefined()
}

/// `$262.agent.sleep(ms)`：阻塞当前线程。
fn sleep(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(millis) = args.first().and_then(|encoded| to_number(state, *encoded)) else {
        return fail_dispatch(ctx);
    };
    let millis = if millis.is_nan() || millis <= 0.0 {
        0
    } else {
        millis.min(f64::from(u32::MAX)) as u64
    };
    std::thread::sleep(Duration::from_millis(millis));
    value::encode_undefined()
}

/// `$262.agent.monotonicNow()`：cluster 基准单调时钟（ms）。
fn monotonic_now(state: &NativeAgentState) -> i64 {
    value::encode_f64(state.node_worker_threads.cluster.agent.monotonic_millis())
}

/// agent 线程入口：编译 script、运行、处理广播直到退出。
fn run_agent(script: String, root: std::path::PathBuf, context: Test262AgentContext) {
    let agent_id = context.agent_id;
    let cluster = Arc::clone(&context.cluster);
    let runtime_config = context.runtime_config.clone();
    let command_rx = context.command_rx;
    let outcome = compile_agent_artifact(&script, &root).and_then(|artifact| {
        let mut runtime = NativeRuntime::new_with_config(runtime_config.clone())
            .map_err(|error| error.to_string())?;
        runtime.configure_test262_agent(Arc::clone(&cluster));
        runtime
            .execute(&artifact, &root, &root)
            .map_err(|error| error.to_string())?;
        runtime.run_test262_agent_loop(&command_rx)
    });
    let _ = outcome;
    // 事件循环未消费的残留广播命令 → 确认，避免 main 的 broadcast 永久阻塞。
    while let Ok(AgentCommand::Broadcast(seq)) = command_rx.try_recv() {
        cluster.agent.confirm_broadcast(seq);
    }
    cluster.agent.unregister_agent(agent_id);
}

/// 把 agent script 编译为 portable artifact（script 模式）。
fn compile_agent_artifact(
    script: &str,
    root: &std::path::Path,
) -> Result<wjsm_artifact_format::PortableArtifact, String> {
    let module = wjsm_parser::parse_script_as_module(script).map_err(|error| error.to_string())?;
    let module_id = wjsm_ir::ModuleId(0);
    let input = wjsm_semantic::ModuleLoweringInput {
        id: module_id,
        ast: module,
        metadata: wjsm_semantic::ModuleMetadata {
            filename: "[test262 agent]".into(),
            dirname: root.display().to_string(),
            url: "test262:agent".into(),
            kind: wjsm_semantic::ModuleKind::CommonJs,
        },
        source: Some(Arc::<str>::from(script)),
    };
    let program = wjsm_semantic::lower_modules(vec![input], wjsm_semantic::ModuleLinking::empty())
        .map_err(|error| error.to_string())?;
    wjsm_artifact_format::PortableArtifact::from_input(
        &wjsm_artifact_format::ArtifactBuildInput::new(
            program,
            wjsm_artifact_format::ModuleManifest::single("test262:agent", true),
            wjsm_artifact_format::BuildOptions::default(),
        ),
    )
    .map_err(|error| error.to_string())
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

// 供 lib.rs 调用的内部接口（避免 crate 级可见性扩散）。

impl NativeRuntime {
    /// 把当前 runtime 配置为 test262 agent（共享 cluster、agent 状态）。
    pub(crate) fn configure_test262_agent(
        &mut self,
        cluster: Arc<super::node_worker_threads::WorkerCluster>,
    ) {
        self.state.node_worker_threads.cluster = cluster;
        self.state.test262_agent = Some(Test262AgentState::new());
        self.state.agent_bridge = None;
    }

    /// test262 agent 事件循环：等待 Broadcast/Leave 命令并分派。
    ///
    /// 广播到达时在 agent 本地 materialize SAB，然后调用 receiveBroadcast
    /// 回调（若已注册），最后确认 retrieve。
    pub(crate) fn run_test262_agent_loop(
        &mut self,
        command_rx: &std::sync::mpsc::Receiver<AgentCommand>,
    ) -> Result<(), String> {
        loop {
            let Ok(AgentCommand::Broadcast(seq)) = command_rx.recv() else {
                // main 已关闭通道：agent 正常退出。
                return Ok(());
            };
            let Some((_, backing_id, value)) =
                self.state.node_worker_threads.cluster.agent.pop_broadcast()
            else {
                self.state
                    .node_worker_threads
                    .cluster
                    .agent
                    .confirm_broadcast(seq);
                continue;
            };
            self.dispatch_agent_broadcast(backing_id, value);
            self.state
                .node_worker_threads
                .cluster
                .agent
                .confirm_broadcast(seq);
        }
    }

    /// 在 agent 本地 materialize SAB 并调用 receiveBroadcast 回调。
    fn dispatch_agent_broadcast(&mut self, backing_id: u32, value: Option<i64>) {
        let Some(callback) = self
            .state
            .test262_agent
            .as_ref()
            .and_then(|agent| agent.receive_callback)
        else {
            return;
        };
        let Some(sab) = self.materialize_sab(backing_id) else {
            return;
        };
        let mut arguments = vec![sab];
        if let Some(value) = value {
            arguments.push(value);
        }
        let ctx = Pin::as_mut(&mut self.vmctx).get_mut();
        let _ = self
            .state
            .invoke_callable(ctx, callback, value::encode_undefined(), &arguments);
    }

    /// 在 agent 本地创建指向 cluster backing 的 SAB 对象（接线 [[Prototype]]）。
    fn materialize_sab(&mut self, backing_id: u32) -> Option<i64> {
        super::sab::materialize_from_backing(&mut self.state, backing_id)
    }
}
