//! AsyncHooksState：async_id 计数、执行栈、hook 列表、context frame、CapturedScope。

use super::context_frame::{AlsKey, ContextFrame, FrameId, FrameTable};
use crate::value;
use std::collections::{HashMap, HashSet, VecDeque};

/// 资源构造/调度时捕获的 scope，fire 时恢复（禁止 fire-time current）。
#[derive(Debug, Clone, Copy)]
pub struct CapturedScope {
    pub async_id: u64,
    pub trigger_async_id: u64,
    pub resource: i64,
    pub frame_id: Option<FrameId>,
}
#[derive(Debug, Clone, Copy)]
pub enum PendingPromiseHookEvent {
    Init {
        scope: CapturedScope,
        type_value: i64,
    },
    Resolve {
        async_id: u64,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AsyncHooksFlags {
    pub hooks_enabled: bool,
    pub als_in_use: bool,
}

#[derive(Debug, Clone)]
pub struct HookRecord {
    pub id: u64,
    pub init: i64,
    pub before: i64,
    pub after: i64,
    pub destroy: i64,
    pub promise_resolve: i64,
    /// false → 跳过 PROMISE 类 hook（Node trackPromises:false）
    pub track_promises: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ResourceMeta {
    pub resource: i64,
    pub manual_destroy: bool,
    pub destroyed: bool,
}

#[derive(Debug, Clone, Copy)]
struct StackEntry {
    execution: u64,
    trigger: u64,
    resource: i64,
}

#[derive(Debug, Default, Clone, Copy)]
struct HookCounts {
    init: u32,
    before: u32,
    after: u32,
    destroy: u32,
    promise_resolve: u32,
}

/// 每 Store 一份 hooks/ALS 状态。
#[derive(Debug)]
pub struct AsyncHooksState {
    async_id_counter: u64,
    execution_async_id: u64,
    trigger_async_id: u64,
    default_trigger_async_id: Option<u64>,
    /// 进入资源前的栈帧（pop 时恢复）
    id_stack: Vec<StackEntry>,
    /// 当前 executionAsyncResource
    current_resource: i64,
    hooks: Vec<HookRecord>,
    next_hook_id: u64,
    hook_counts: HookCounts,
    emit_depth: u32,
    pending_hooks: Option<Vec<HookRecord>>,
    pending_hook_enable_changes: Vec<(u64, bool)>,
    destroy_queue: VecDeque<u64>,
    pending_promise_events: VecDeque<PendingPromiseHookEvent>,
    promise_type_value: Option<i64>,
    resources: HashMap<u64, ResourceMeta>,
    frames: FrameTable,
    current_frame: Option<FrameId>,
    retained_frames: HashSet<FrameId>,
    next_als_key: AlsKey,
    als_default_values: HashMap<AlsKey, i64>,
    flags: AsyncHooksFlags,
    enabled_als_keys: HashSet<AlsKey>,
    top_level_resource: i64,
    fatal_in_progress: bool,
}

impl AsyncHooksState {
    /// Node 风格 bootstrap：execution=1, trigger=0，下一 id 从 2 起。
    pub fn bootstrap() -> Self {
        Self {
            async_id_counter: 1,
            execution_async_id: 1,
            trigger_async_id: 0,
            default_trigger_async_id: None,
            id_stack: Vec::new(),
            current_resource: 0,
            hooks: Vec::new(),
            next_hook_id: 1,
            hook_counts: HookCounts::default(),
            emit_depth: 0,
            pending_hooks: None,
            pending_hook_enable_changes: Vec::new(),
            destroy_queue: VecDeque::new(),
            resources: HashMap::new(),
            pending_promise_events: VecDeque::new(),
            promise_type_value: None,
            frames: FrameTable::new(),
            current_frame: None,
            retained_frames: HashSet::new(),
            next_als_key: 1,
            als_default_values: HashMap::new(),
            flags: AsyncHooksFlags::default(),
            enabled_als_keys: HashSet::new(),
            top_level_resource: 0,
            fatal_in_progress: false,
        }
    }

    pub fn is_empty_for_snapshot(&self) -> bool {
        self.async_id_counter == 1
            && self.execution_async_id == 1
            && self.trigger_async_id == 0
            && self.default_trigger_async_id.is_none()
            && self.id_stack.is_empty()
            && self.current_resource == 0
            && self.hooks.is_empty()
            && self.next_hook_id == 1
            && self.emit_depth == 0
            && self.pending_hooks.is_none()
            && self.pending_hook_enable_changes.is_empty()
            && self.destroy_queue.is_empty()
            && self.pending_promise_events.is_empty()
            && self.promise_type_value.is_none()
            && self.resources.is_empty()
            && self.frames.is_empty()
            && self.current_frame.is_none()
            && self.retained_frames.is_empty()
            && self.next_als_key == 1
            && self.als_default_values.is_empty()
            && !self.flags.hooks_enabled
            && self.enabled_als_keys.is_empty()
            && !self.flags.als_in_use
            && self.top_level_resource == 0
            && !self.fatal_in_progress
    }

    pub fn gc_roots(&self) -> Vec<i64> {
        let mut roots = Vec::new();
        roots.extend([self.top_level_resource, self.current_resource]);
        roots.extend(self.id_stack.iter().map(|entry| entry.resource));
        roots.extend(self.hooks.iter().flat_map(|hook| {
            [
                hook.init,
                hook.before,
                hook.after,
                hook.destroy,
                hook.promise_resolve,
            ]
        }));
        roots.extend(self.frames.values());
        roots.extend(self.als_default_values.values().copied());
        roots.extend(self.promise_type_value);
        for event in &self.pending_promise_events {
            if let PendingPromiseHookEvent::Init { scope, type_value } = event {
                roots.extend([scope.resource, *type_value]);
            }
        }
        roots
    }

    pub fn execution_async_id(&self) -> u64 {
        self.execution_async_id
    }

    pub fn trigger_async_id(&self) -> u64 {
        self.trigger_async_id
    }

    pub fn peek_next_async_id(&self) -> u64 {
        self.async_id_counter.saturating_add(1)
    }

    /// 先 ++ 再返回（对齐 Node newAsyncId）。
    pub fn new_async_id(&mut self) -> u64 {
        self.async_id_counter = self.async_id_counter.saturating_add(1);
        self.async_id_counter
    }

    pub fn default_trigger_async_id(&self) -> u64 {
        self.default_trigger_async_id
            .unwrap_or(self.execution_async_id)
    }

    pub fn set_default_trigger_async_id(&mut self, id: Option<u64>) {
        self.default_trigger_async_id = id;
    }

    pub fn flags(&self) -> AsyncHooksFlags {
        self.flags
    }

    pub fn hooks_or_als_active(&self) -> bool {
        self.flags.hooks_enabled || self.flags.als_in_use
    }

    pub fn set_top_level_resource(&mut self, resource: i64) {
        self.top_level_resource = resource;
        if self.current_resource == 0 {
            self.current_resource = resource;
        }
    }

    pub fn execution_async_resource(&self) -> i64 {
        if self.current_resource != 0 {
            self.current_resource
        } else {
            self.top_level_resource
        }
    }

    pub fn push_async_context(&mut self, async_id: u64, trigger_async_id: u64, resource: i64) {
        self.id_stack.push(StackEntry {
            execution: self.execution_async_id,
            trigger: self.trigger_async_id,
            resource: self.current_resource,
        });
        self.execution_async_id = async_id;
        self.trigger_async_id = trigger_async_id;
        self.current_resource = resource;
    }

    /// 成功 pop 返回 true。
    pub fn pop_async_context(&mut self, expected_async_id: u64) -> bool {
        if self.execution_async_id != expected_async_id && !self.id_stack.is_empty() {
            // Node kCheck：不匹配时硬失败；此处返回 false 由调用方处理
            return false;
        }
        if let Some(prev) = self.id_stack.pop() {
            self.execution_async_id = prev.execution;
            self.trigger_async_id = prev.trigger;
            self.current_resource = prev.resource;
            true
        } else {
            false
        }
    }

    pub fn current_frame(&self) -> Option<FrameId> {
        self.current_frame
    }

    pub fn set_current_frame(&mut self, frame: Option<FrameId>) {
        let current = self.current_frame;
        self.current_frame = frame;
        if let Some(current) = current
            && Some(current) != frame
            && !self.retained_frames.contains(&current)
        {
            self.frames.remove(current);
        }
    }

    pub fn retain_current_frame(&mut self) -> Option<FrameId> {
        let frame_id = self.current_frame?;
        self.retained_frames.insert(frame_id);
        Some(frame_id)
    }

    fn replace_current_frame(&mut self, next: FrameId) {
        let prior = self.current_frame.replace(next);
        if let Some(prior) = prior
            && !self.retained_frames.contains(&prior)
        {
            self.frames.remove(prior);
        }
    }

    pub fn alloc_als_key(&mut self, default_value: i64) -> AlsKey {
        let key = self.next_als_key;
        self.next_als_key = self.next_als_key.saturating_add(1);
        self.als_default_values.insert(key, default_value);
        key
    }

    pub fn get_store(&self, key: AlsKey) -> Option<i64> {
        if let Some(fid) = self.current_frame
            && let Some(frame) = self.frames.get_ref(fid)
            && frame.has(key)
        {
            return frame.get(key);
        }
        self.als_default_values.get(&key).copied()
    }

    pub fn frame_get(&self, frame_id: FrameId, key: AlsKey) -> Option<i64> {
        self.frames.get_ref(frame_id).and_then(|f| f.get(key))
    }

    /// enterWith：从 current 派生新 frame 并设为 current，返回新 frame id。
    pub fn enter_with_store(&mut self, key: AlsKey, value: i64) -> FrameId {
        let base = self
            .current_frame
            .and_then(|id| self.frames.get(id))
            .map(|a| (*a).clone())
            .unwrap_or_else(ContextFrame::empty);
        let child = base.child_with(key, value);
        let id = self.frames.alloc(child);
        self.enabled_als_keys.insert(key);
        self.flags.als_in_use = true;
        self.replace_current_frame(id);
        id
    }

    /// 调度/发起回调时捕获；需要资源身份的边界始终分配新 async id，避免
    /// ALS-only completion 与当前 execution stack 复用同一 id。
    pub fn capture_for_scheduled_callback(
        &mut self,
        resource: i64,
        alloc_new_id: bool,
    ) -> Option<CapturedScope> {
        if !self.hooks_or_als_active() {
            return None;
        }
        let async_id = if alloc_new_id {
            self.new_async_id()
        } else {
            self.execution_async_id
        };
        let trigger = self.default_trigger_async_id();
        let resource = if resource != 0 {
            resource
        } else {
            self.execution_async_resource()
        };
        Some(self.capture_scope(async_id, trigger, resource))
    }

    pub fn capture_promise_scope(
        &mut self,
        resource: i64,
        trigger_async_id: Option<u64>,
    ) -> Option<CapturedScope> {
        if !self.hooks_or_als_active() {
            return None;
        }
        let async_id = self.new_async_id();
        let trigger = trigger_async_id.unwrap_or_else(|| self.default_trigger_async_id());
        Some(self.capture_scope(async_id, trigger, resource))
    }

    pub fn queue_promise_event(&mut self, event: PendingPromiseHookEvent) {
        if self.flags.hooks_enabled {
            self.pending_promise_events.push_back(event);
        }
    }

    pub fn take_promise_events(&mut self) -> VecDeque<PendingPromiseHookEvent> {
        std::mem::take(&mut self.pending_promise_events)
    }

    /// disable(als)：从 current 删除 key。
    pub fn disable_store(&mut self, key: AlsKey) {
        let base = self
            .current_frame
            .and_then(|id| self.frames.get(id))
            .map(|a| (*a).clone())
            .unwrap_or_else(ContextFrame::empty);
        let child = base.child_without(key);
        let id = self.frames.alloc(child);
        self.enabled_als_keys.remove(&key);
        self.flags.als_in_use = !self.enabled_als_keys.is_empty();
        self.replace_current_frame(id);
    }

    /// 在调度点捕获 scope（P0-5）。
    pub fn capture_scope(
        &mut self,
        async_id: u64,
        trigger_async_id: u64,
        resource: i64,
    ) -> CapturedScope {
        if let Some(frame_id) = self.current_frame {
            self.retained_frames.insert(frame_id);
        }
        CapturedScope {
            frame_id: self.current_frame,
            async_id,
            trigger_async_id,
            resource,
        }
    }

    /// 进入已捕获 scope；返回需在退出时恢复的 prior frame。
    pub fn enter_captured_scope(&mut self, scope: CapturedScope) -> Option<FrameId> {
        let prior = self.current_frame;
        if let Some(prior) = prior {
            self.retained_frames.insert(prior);
        }
        self.push_async_context(scope.async_id, scope.trigger_async_id, scope.resource);
        self.current_frame = scope.frame_id;
        prior
    }

    pub fn exit_captured_scope(&mut self, scope: CapturedScope, prior: Option<FrameId>) {
        self.current_frame = prior;
        self.pop_async_context(scope.async_id);
    }

    pub fn register_resource(&mut self, async_id: u64, meta: ResourceMeta) {
        self.resources.insert(async_id, meta);
    }

    pub fn queue_destroy(&mut self, async_id: u64) {
        if async_id == 0 {
            return;
        }
        let Some(resource) = self.resources.get_mut(&async_id) else {
            return;
        };
        if resource.destroyed {
            return;
        }
        resource.destroyed = true;
        self.destroy_queue.push_back(async_id);
    }

    pub fn take_destroy_queue(&mut self) -> VecDeque<u64> {
        std::mem::take(&mut self.destroy_queue)
    }

    pub fn queue_auto_destroy_for_freed(&mut self, freed_handles: &HashSet<u32>) {
        let async_ids = self
            .resources
            .iter()
            .filter_map(|(&async_id, meta)| {
                (value::is_object(meta.resource)
                    && freed_handles.contains(&value::decode_object_handle(meta.resource)))
                .then_some(async_id)
            })
            .collect::<Vec<_>>();
        for async_id in async_ids {
            if let Some(meta) = self.resources.remove(&async_id)
                && !meta.manual_destroy
                && !meta.destroyed
            {
                self.destroy_queue.push_back(async_id);
            }
        }
    }

    pub fn fatal_in_progress(&self) -> bool {
        self.fatal_in_progress
    }

    pub fn set_fatal_in_progress(&mut self, v: bool) {
        self.fatal_in_progress = v;
    }

    // ── hooks 列表（Phase 2 完整；Phase 1 可注册） ──

    pub fn begin_emit(&mut self) {
        self.emit_depth = self.emit_depth.saturating_add(1);
        if self.emit_depth == 1 {
            self.pending_hooks = None;
            self.pending_hook_enable_changes.clear();
        }
    }

    pub fn end_emit(&mut self) {
        self.emit_depth = self.emit_depth.saturating_sub(1);
        if self.emit_depth != 0 {
            return;
        }
        if let Some(pending) = self.pending_hooks.take() {
            self.hooks = pending;
        }
        for (id, enabled) in self.pending_hook_enable_changes.drain(..) {
            if let Some(record) = self.hooks.iter_mut().find(|hook| hook.id == id) {
                record.enabled = enabled;
            }
        }
        self.recompute_hook_counts();
    }

    fn hooks_mut_target(&mut self) -> &mut Vec<HookRecord> {
        if self.emit_depth > 0 {
            if self.pending_hooks.is_none() {
                self.pending_hooks = Some(self.hooks.clone());
            }
            self.pending_hooks.as_mut().expect("pending hooks")
        } else {
            &mut self.hooks
        }
    }

    pub fn register_hook(&mut self, mut record: HookRecord) -> u64 {
        let id = self.next_hook_id;
        self.next_hook_id = self.next_hook_id.saturating_add(1);
        record.id = id;
        self.hooks_mut_target().push(record);
        if self.emit_depth == 0 {
            self.recompute_hook_counts();
        }
        id
    }

    pub fn set_hook_enabled(&mut self, id: u64, enabled: bool) {
        if self.emit_depth > 0 {
            self.pending_hook_enable_changes.push((id, enabled));
            return;
        }
        if let Some(record) = self.hooks.iter_mut().find(|hook| hook.id == id) {
            record.enabled = enabled;
        }
        self.recompute_hook_counts();
    }

    pub fn promise_type_value(&self) -> Option<i64> {
        self.promise_type_value
    }

    pub fn set_promise_type_value(&mut self, value: i64) {
        self.promise_type_value = Some(value);
    }

    pub fn active_hooks(&self) -> &[HookRecord] {
        &self.hooks
    }

    fn recompute_hook_counts(&mut self) {
        let mut c = HookCounts::default();
        let mut any = false;
        for h in &self.hooks {
            if !h.enabled {
                continue;
            }
            any = true;
            if h.init != 0 {
                c.init += 1;
            }
            if h.before != 0 {
                c.before += 1;
            }
            if h.after != 0 {
                c.after += 1;
            }
            if h.destroy != 0 {
                c.destroy += 1;
            }
            if h.promise_resolve != 0 {
                c.promise_resolve += 1;
            }
        }
        self.hook_counts = c;
        self.flags.hooks_enabled = any;
    }

    pub fn init_hooks_exist(&self) -> bool {
        self.hook_counts.init > 0
    }
}
