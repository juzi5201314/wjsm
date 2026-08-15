//! 运行时类型反馈特化的后台编译与 overlay 选择表。
//!
//! worker 只接触 compiler、`Arc<Program>` 与变量槽快照；RX image 的加载、发布、
//! 淘汰和 activation pin 均由 owner thread 上的 `NativeAgentState` 完成。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};

use wjsm_backend_native::image::CompiledImage;
use wjsm_backend_native::{NativeCompiler, NativeObject};
use wjsm_ir::{FunctionId, Program, constants};
use wjsm_native_abi::{NativeFeedbackSlot, NativeFeedbackTag};

const REQUEST_QUEUE_CAPACITY: usize = 8;
const MAX_ACTIVE_OVERLAYS: usize = 64;
const MAX_ACTIVE_OVERLAY_BYTES: usize = 16 * 1024 * 1024;

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

pub(crate) struct SpecializationCoordinator {
    compiler: Option<NativeCompiler>,
    request_tx: Option<SyncSender<CompilationRequest>>,
    result_rx: Option<Receiver<CompilationResult>>,
    worker: Option<JoinHandle<()>>,
    pending: HashSet<VariantKey>,
    disabled: HashSet<VariantKey>,
    overlays: HashMap<VariantKey, PublishedOverlay>,
    active_bytes: usize,
    tick: u64,
    next_overlay_image_id: u64,
}

impl SpecializationCoordinator {
    pub(crate) fn new(compiler: NativeCompiler) -> Self {
        Self {
            compiler: Some(compiler),
            request_tx: None,
            result_rx: None,
            worker: None,
            pending: HashSet::new(),
            disabled: HashSet::new(),
            overlays: HashMap::new(),
            active_bytes: 0,
            tick: 0,
            next_overlay_image_id: u64::MAX,
        }
    }

    fn ensure_worker(&mut self) -> Option<&SyncSender<CompilationRequest>> {
        if self.request_tx.is_none() {
            let compiler = self.compiler.take()?;
            let (request_tx, request_rx) =
                mpsc::sync_channel::<CompilationRequest>(REQUEST_QUEUE_CAPACITY);
            let (result_tx, result_rx) = mpsc::channel::<CompilationResult>();
            let worker = thread::Builder::new()
                .name("wjsm-specialization".into())
                .spawn(move || {
                    while let Ok(request) = request_rx.recv() {
                        let object = compiler
                            .compile_specialized_function(
                                &request.program,
                                &request.variable_slots,
                                FunctionId(request.key.target_function),
                                &request.argument_tags,
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
                    }
                })
                .ok()?;
            self.request_tx = Some(request_tx);
            self.result_rx = Some(result_rx);
            self.worker = Some(worker);
        }
        self.request_tx.as_ref()
    }

    pub(crate) fn enqueue(&mut self, request: CompilationRequest) {
        if self.pending.contains(&request.key) || self.disabled.contains(&request.key) {
            return;
        }
        let key = request.key;
        let Some(sender) = self.ensure_worker() else {
            self.disabled.insert(key);
            return;
        };
        match sender.try_send(request) {
            Ok(()) => {
                self.pending.insert(key);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                self.disabled.insert(key);
            }
        }
    }

    pub(crate) fn drain_results(&mut self) -> Vec<CompilationResult> {
        let mut results = Vec::new();
        let Some(receiver) = &self.result_rx else {
            return results;
        };
        loop {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending.remove(&result.request.key);
                    if result.object.is_none() {
                        self.disabled.insert(result.request.key);
                    }
                    results.push(result);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
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
        while self.overlays.len() > MAX_ACTIVE_OVERLAYS
            || self.active_bytes > MAX_ACTIVE_OVERLAY_BYTES
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
        }
    }
}

impl Drop for SpecializationCoordinator {
    fn drop(&mut self) {
        self.request_tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
