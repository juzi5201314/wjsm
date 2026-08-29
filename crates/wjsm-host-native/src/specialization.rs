//! 运行时类型反馈特化的后台编译与 overlay 选择表。
//!
//! worker 只接触 compiler、`Arc<Program>` 与变量槽快照；RX image 的加载、发布、
//! 淘汰和 activation pin 均由 owner thread 上的 `NativeAgentState` 完成。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use wjsm_backend_native::image::CompiledImage;
use wjsm_backend_native::{NativeCompiler, NativeObject};
use wjsm_ir::{FunctionId, Program, ValueId, constants};
use wjsm_native_abi::{NativeFeedbackSlot, NativeFeedbackTag};
use wjsm_optimize::SpeculativeFacts;

const REQUEST_QUEUE_CAPACITY: usize = 256;

fn overlay_count_limit() -> usize {
    match std::env::var("WJSM_OVERLAY_MAX_COUNT") {
        Ok(value) => {
            let parsed = value.parse::<usize>().unwrap_or(4096);
            if parsed == 0 { usize::MAX } else { parsed }
        }
        Err(_) => 4096,
    }
}

fn overlay_byte_limit() -> usize {
    match std::env::var("WJSM_OVERLAY_MAX_BYTES") {
        Ok(value) => {
            let parsed = value.parse::<usize>().unwrap_or(0);
            if parsed == 0 { usize::MAX } else { parsed }
        }
        Err(_) => {
            let rss = current_rss_bytes().unwrap_or(256 * 1024 * 1024);
            (rss / 8).clamp(32 * 1024 * 1024, 256 * 1024 * 1024)
        }
    }
}

fn current_rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(kb) = line.strip_prefix("VmRSS:") else {
            continue;
        };
        let kb = kb.trim().trim_end_matches(" kB").trim();
        let kb = kb.parse::<usize>().ok()?;
        return Some(kb.saturating_mul(1024));
    }
    None
}

#[derive(Clone, Copy)]
pub(crate) struct ValidatedFeedbackSlot {
    pointer: *mut NativeFeedbackSlot,
    pub(crate) caller_image_id: u64,
    pub(crate) site_index: u32,
}

impl ValidatedFeedbackSlot {
    pub(crate) fn new(
        pointer: *mut NativeFeedbackSlot,
        caller_image_id: u64,
        site_index: u32,
    ) -> Self {
        Self {
            pointer,
            caller_image_id,
            site_index,
        }
    }

    pub(crate) fn slot(self) -> *mut NativeFeedbackSlot {
        self.pointer
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct VariantKey {
    pub(crate) caller_image_id: u64,
    pub(crate) site_index: u32,
    pub(crate) target_image_id: u64,
    pub(crate) target_function: u32,
    pub(crate) tag_signature: u64,
}

impl VariantKey {
    fn same_site(self, other: Self) -> bool {
        self.caller_image_id == other.caller_image_id
            && self.site_index == other.site_index
            && self.target_image_id == other.target_image_id
            && self.target_function == other.target_function
    }
}

pub(crate) struct CompilationRequest {
    pub(crate) key: VariantKey,
    pub(crate) program: Arc<Program>,
    pub(crate) variable_slots: Arc<HashMap<String, u32>>,
    pub(crate) argument_tags: Box<[NativeFeedbackTag]>,
    pub(crate) extra_numbers: HashSet<ValueId>,
    pub(crate) facts: SpeculativeFacts,
    pub(crate) ic_epoch: u64,
    pub(crate) proto_generation: u64,
}

pub(crate) struct CompilationResult {
    pub(crate) request: CompilationRequest,
    pub(crate) object: Option<NativeObject>,
}

struct PublishedOverlay {
    image: Arc<CompiledImage>,
    byte_size: usize,
    last_used: u64,
    ic_epoch: u64,
    proto_generation: u64,
}

struct Inbox {
    queue: Mutex<VecDeque<CompilationRequest>>,
    cv: Condvar,
    closed: AtomicBool,
}

pub(crate) struct SpecializationCoordinator {
    compiler: Option<NativeCompiler>,
    inbox: Option<Arc<Inbox>>,
    result_rx: Option<Receiver<CompilationResult>>,
    worker: Option<JoinHandle<()>>,
    pending: HashSet<VariantKey>,
    disabled: HashSet<VariantKey>,
    overlays: HashMap<VariantKey, PublishedOverlay>,
    /// worker 已投递、owner 尚未收敛的结果数。
    ///
    /// owner 在每一次宿主调用的入口都要判断有无待收敛结果；没有这个计数就得
    /// 无条件 `try_recv` 并搬动整个协调器，等于让后台分层编译给每一次宿主调用
    /// 收税。放松序的读取足够：漏读一轮只会把收敛推迟到下一次调用。
    completed: Arc<AtomicUsize>,
    active_bytes: usize,
    tick: u64,
    next_overlay_image_id: u64,
    invalidated_osr: Vec<(u64, u32)>,
}

impl SpecializationCoordinator {
    pub(crate) fn new(compiler: NativeCompiler) -> Self {
        Self {
            compiler: Some(compiler),
            inbox: None,
            result_rx: None,
            worker: None,
            pending: HashSet::new(),
            disabled: HashSet::new(),
            overlays: HashMap::new(),
            completed: Arc::new(AtomicUsize::new(0)),
            active_bytes: 0,
            tick: 0,
            next_overlay_image_id: u64::MAX,
            invalidated_osr: Vec::new(),
        }
    }

    fn ensure_worker(&mut self) -> Option<Arc<Inbox>> {
        if self.inbox.is_none() {
            let compiler = self.compiler.take()?;
            let inbox = Arc::new(Inbox {
                queue: Mutex::new(VecDeque::new()),
                cv: Condvar::new(),
                closed: AtomicBool::new(false),
            });
            let (result_tx, result_rx) = mpsc::channel::<CompilationResult>();
            let completed = Arc::clone(&self.completed);
            let worker_inbox = Arc::clone(&inbox);
            let worker = thread::Builder::new()
                .name("wjsm-specialization".into())
                .spawn(move || {
                    loop {
                        let request = {
                            let mut queue = worker_inbox.queue.lock().unwrap();
                            loop {
                                if let Some(request) = queue.pop_front() {
                                    break request;
                                }
                                if worker_inbox.closed.load(Ordering::Acquire) {
                                    return;
                                }
                                queue = worker_inbox.cv.wait(queue).unwrap();
                            }
                        };
                        let object = compiler
                            .compile_specialized_function(
                                &request.program,
                                &request.variable_slots,
                                FunctionId(request.key.target_function),
                                &request.argument_tags,
                                &request.extra_numbers,
                                Some(request.facts.clone()),
                                false,
                            )
                            .ok()
                            .map(|diagnostics| diagnostics.object);
                        if result_tx
                            .send(CompilationResult { request, object })
                            .is_err()
                        {
                            break;
                        }
                        completed.fetch_add(1, Ordering::Release);
                    }
                })
                .ok()?;
            self.inbox = Some(inbox);
            self.result_rx = Some(result_rx);
            self.worker = Some(worker);
        }
        self.inbox.clone()
    }

    pub(crate) fn enqueue(&mut self, request: CompilationRequest) {
        if self.pending.contains(&request.key) || self.disabled.contains(&request.key) {
            return;
        }
        let key = request.key;
        let Some(inbox) = self.ensure_worker() else {
            self.disabled.insert(key);
            return;
        };
        let mut queue = inbox.queue.lock().unwrap();
        queue.retain(|pending| pending.key != key);
        while queue.len() >= REQUEST_QUEUE_CAPACITY {
            if let Some(oldest) = queue.pop_front() {
                self.pending.remove(&oldest.key);
            } else {
                break;
            }
        }
        queue.push_back(request);
        self.pending.insert(key);
        inbox.cv.notify_one();
    }

    /// 是否有 worker 已投递、尚未收敛的结果。
    pub(crate) fn has_results(&self) -> bool {
        self.completed.load(Ordering::Acquire) != 0
    }

    pub(crate) fn drain_results(&mut self) -> Vec<CompilationResult> {
        let mut results = Vec::new();
        let Some(receiver) = &self.result_rx else {
            return results;
        };
        while let Ok(result) = receiver.try_recv() {
            self.pending.remove(&result.request.key);
            if result.object.is_none() {
                self.disabled.insert(result.request.key);
            }
            results.push(result);
        }
        // 计数只由本方法回落；worker 只增不减，二者不会互相覆盖。
        self.completed.fetch_sub(results.len(), Ordering::AcqRel);
        results
    }
    pub(crate) fn reset_runtime_state(&mut self) {
        self.pending.clear();
        self.disabled.clear();
        self.overlays.clear();
        self.active_bytes = 0;
    }

    pub(crate) fn next_image_id(&mut self) -> u64 {
        let image_id = self.next_overlay_image_id;
        self.next_overlay_image_id = self.next_overlay_image_id.saturating_sub(1);
        image_id
    }

    pub(crate) fn publish(
        &mut self,
        key: VariantKey,
        image: Arc<CompiledImage>,
        ic_epoch: u64,
        proto_generation: u64,
    ) {
        self.tick = self.tick.wrapping_add(1);
        let byte_size = image.code_bytes().saturating_add(image.rodata_bytes());
        if let Some(previous) = self.overlays.remove(&key) {
            self.active_bytes = self.active_bytes.saturating_sub(previous.byte_size);
        }
        self.active_bytes = self.active_bytes.saturating_add(byte_size);
        self.overlays.insert(
            key,
            PublishedOverlay {
                image,
                byte_size,
                last_used: self.tick,
                ic_epoch,
                proto_generation,
            },
        );
        self.enforce_site_limit(key);
        self.enforce_global_limits();
    }

    pub(crate) fn select(
        &mut self,
        key: VariantKey,
        ic_epoch: u64,
        proto_generation: u64,
    ) -> Option<Arc<CompiledImage>> {
        let invalid = self.overlays.get(&key).is_some_and(|overlay| {
            overlay.ic_epoch != ic_epoch || overlay.proto_generation != proto_generation
        });
        if invalid {
            self.remove_site(key);
            return None;
        }
        self.tick = self.tick.wrapping_add(1);
        let overlay = self.overlays.get_mut(&key)?;
        overlay.last_used = self.tick;
        Some(Arc::clone(&overlay.image))
    }

    fn enforce_site_limit(&mut self, site: VariantKey) {
        while self
            .overlays
            .keys()
            .filter(|key| key.same_site(site))
            .count()
            > usize::try_from(constants::FEEDBACK_MAX_VARIANTS_PER_SITE)
                .expect("特化版本上限在 usize 内")
        {
            let Some(oldest) = self
                .overlays
                .iter()
                .filter(|(key, _)| key.same_site(site))
                .min_by_key(|(_, overlay)| overlay.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.remove(oldest);
        }
    }

    fn enforce_global_limits(&mut self) {
        while self.overlays.len() > overlay_count_limit()
            || self.active_bytes > overlay_byte_limit()
        {
            let Some(oldest) = self
                .overlays
                .iter()
                .min_by_key(|(_, overlay)| overlay.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.remove(oldest);
        }
    }

    fn remove_site(&mut self, site: VariantKey) {
        let keys = self
            .overlays
            .keys()
            .copied()
            .filter(|key| key.same_site(site))
            .collect::<Vec<_>>();
        for key in keys {
            self.remove(key);
        }
    }

    fn remove(&mut self, key: VariantKey) {
        if let Some(overlay) = self.overlays.remove(&key) {
            self.active_bytes = self.active_bytes.saturating_sub(overlay.byte_size);
            self.invalidated_osr
                .push((key.target_image_id, key.target_function));
        }
    }

    pub(crate) fn disable_target_function(&mut self, function: u32) {
        let keys: Vec<VariantKey> = self
            .overlays
            .keys()
            .copied()
            .filter(|key| key.target_function == function)
            .collect();
        for key in keys {
            self.disabled.insert(key);
            self.remove(key);
        }
    }

    pub(crate) fn take_osr_invalidations(&mut self) -> Vec<(u64, u32)> {
        std::mem::take(&mut self.invalidated_osr)
    }
}

impl Drop for SpecializationCoordinator {
    fn drop(&mut self) {
        if let Some(inbox) = &self.inbox {
            inbox.closed.store(true, Ordering::Release);
            inbox.cv.notify_all();
        }
        self.inbox.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
