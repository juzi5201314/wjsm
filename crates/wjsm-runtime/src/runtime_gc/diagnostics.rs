//! GC 可观测性与诊断报告。
//!
//! 本模块只收集运行时已经拥有的事实：GC cycle 统计、堆对象快照、host 侧表
//! 引用来源和 allocation profile。默认执行路径保持关闭，`--trace-gc` 或显式
//! RuntimeExecutionOptions 打开后才记录 allocation profile 和构建快照。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use wasmparser::{Parser, Payload};
use wasmtime::Store;
use wjsm_ir::{constants, value};

use crate::runtime_gc::api::GcStats;
use crate::runtime_gc::context::{heap_type_from_memory, object_size_from_memory};
use crate::{Microtask, PromiseReactionKind, PromiseState, RuntimeState, WasmEnv};

pub const ALLOCATION_SITES_SECTION: &str = "wjsm.gc.alloc_sites";
const ALLOCATION_SITES_MAGIC: &[u8; 8] = b"WJSMAS01";
const ALLOCATION_SITES_VERSION: u32 = 1;

pub(crate) const RUNTIME_ALLOCATION_SITE_ID: u32 = 0;
pub(crate) const HOST_ALLOCATION_SITE_ID: u32 = 1;
const FIRST_COMPILER_ALLOCATION_SITE_ID: u32 = 2;

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeExecutionOptions {
    pub max_heap_size: Option<usize>,
    pub gc: GcDiagnosticsOptions,
}

impl RuntimeExecutionOptions {
    pub fn trace_gc() -> Self {
        Self {
            max_heap_size: None,
            gc: GcDiagnosticsOptions::trace_all(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GcDiagnosticsOptions {
    pub trace: bool,
    pub heap_snapshot: bool,
    pub allocation_profile: bool,
}

impl GcDiagnosticsOptions {
    pub fn trace_all() -> Self {
        Self {
            trace: true,
            heap_snapshot: true,
            allocation_profile: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeExecutionReport {
    pub gc: GcDiagnosticsReport,
}

#[derive(Debug, Clone, Default)]
pub struct GcDiagnosticsReport {
    pub cycles: Vec<GcCycleReport>,
    pub summary: GcSummary,
    pub heap_snapshot: Option<HeapSnapshot>,
    pub allocation_profile: Vec<AllocationProfileEntry>,
}

#[derive(Debug, Clone)]
pub struct GcCycleReport {
    pub cycle: u64,
    pub step: u64,
    pub trigger: String,
    pub algorithm: String,
    pub marked: usize,
    pub swept: usize,
    pub freed_bytes: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct GcSummary {
    pub cycles: u64,
    pub total_marked: usize,
    pub total_swept: usize,
    pub total_freed_bytes: usize,
    pub total_elapsed: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct HeapSnapshot {
    pub objects: Vec<HeapObjectSnapshot>,
    pub edges: Vec<HeapEdgeSnapshot>,
    pub host_refs: Vec<HeapHostReference>,
    pub total_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct HeapObjectSnapshot {
    pub handle: u32,
    pub ptr: usize,
    pub type_name: String,
    pub size: usize,
    pub capacity: u32,
    pub used_slots: u32,
}

#[derive(Debug, Clone)]
pub struct HeapEdgeSnapshot {
    pub from: u32,
    pub label: String,
    pub to: Option<u32>,
    pub value_tag: String,
}

#[derive(Debug, Clone)]
pub struct HeapHostReference {
    pub source: String,
    pub to: Option<u32>,
    pub value_tag: String,
}

#[derive(Debug, Clone)]
pub struct AllocationProfileEntry {
    pub site_id: u32,
    pub function_id: Option<u32>,
    pub function_name: String,
    pub block: Option<u32>,
    pub instruction: Option<u32>,
    pub kind: String,
    pub count: u64,
    pub bytes: u64,
    pub capacity_total: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AllocationSiteRegistry {
    sites: BTreeMap<u32, AllocationSite>,
}

impl AllocationSiteRegistry {
    pub(crate) fn from_wasm(wasm_bytes: &[u8]) -> Self {
        let mut registry = Self::with_builtin_sites();
        for payload in Parser::new(0).parse_all(wasm_bytes) {
            let Ok(Payload::CustomSection(section)) = payload else {
                continue;
            };
            if section.name() == ALLOCATION_SITES_SECTION {
                registry.merge_section(section.data());
            }
        }
        registry
    }

    fn with_builtin_sites() -> Self {
        let mut registry = Self::default();
        registry.sites.insert(
            RUNTIME_ALLOCATION_SITE_ID,
            AllocationSite::builtin(RUNTIME_ALLOCATION_SITE_ID, "<runtime>"),
        );
        registry.sites.insert(
            HOST_ALLOCATION_SITE_ID,
            AllocationSite::builtin(HOST_ALLOCATION_SITE_ID, "<host>"),
        );
        registry
    }

    fn merge_section(&mut self, data: &[u8]) {
        let mut cursor = SectionCursor::new(data);
        if cursor.read_bytes(ALLOCATION_SITES_MAGIC.len()) != Some(ALLOCATION_SITES_MAGIC) {
            return;
        }
        if cursor.read_u32() != Some(ALLOCATION_SITES_VERSION) {
            return;
        }
        let Some(count) = cursor.read_u32() else {
            return;
        };
        for _ in 0..count {
            let Some(site_id) = cursor.read_u32() else {
                return;
            };
            let Some(function_id_raw) = cursor.read_u32() else {
                return;
            };
            let Some(block_raw) = cursor.read_u32() else {
                return;
            };
            let Some(instruction_raw) = cursor.read_u32() else {
                return;
            };
            let Some(kind_byte) = cursor.read_u8() else {
                return;
            };
            let Some(name) = cursor.read_string() else {
                return;
            };
            if site_id < FIRST_COMPILER_ALLOCATION_SITE_ID {
                continue;
            }
            self.sites.insert(
                site_id,
                AllocationSite {
                    site_id,
                    function_id: decode_optional_u32(function_id_raw),
                    function_name: name,
                    block: decode_optional_u32(block_raw),
                    instruction: decode_optional_u32(instruction_raw),
                    kind: allocation_site_kind_name(kind_byte).to_string(),
                },
            );
        }
    }

    fn describe(&self, site_id: u32, heap_type: u8) -> AllocationSite {
        self.sites
            .get(&site_id)
            .cloned()
            .unwrap_or_else(|| AllocationSite {
                site_id,
                function_id: None,
                function_name: format!("<site:{site_id}>"),
                block: None,
                instruction: None,
                kind: heap_type_name(heap_type).to_string(),
            })
    }
}

#[derive(Debug, Clone)]
struct AllocationSite {
    site_id: u32,
    function_id: Option<u32>,
    function_name: String,
    block: Option<u32>,
    instruction: Option<u32>,
    kind: String,
}

impl AllocationSite {
    fn builtin(site_id: u32, name: &str) -> Self {
        Self {
            site_id,
            function_id: None,
            function_name: name.to_string(),
            block: None,
            instruction: None,
            kind: "runtime".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd)]
struct AllocationProfileKey {
    site_id: u32,
    heap_type: u8,
}

#[derive(Debug, Clone, Default)]
struct AllocationProfileBucket {
    count: u64,
    bytes: u64,
    capacity_total: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct GcDiagnosticsState {
    options: GcDiagnosticsOptions,
    allocation_sites: AllocationSiteRegistry,
    next_cycle: u64,
    cycles: Vec<GcCycleReport>,
    pending_allocation_site: u32,
    allocation_profile: BTreeMap<AllocationProfileKey, AllocationProfileBucket>,
}

impl Default for GcDiagnosticsState {
    fn default() -> Self {
        Self {
            options: GcDiagnosticsOptions::default(),
            allocation_sites: AllocationSiteRegistry::with_builtin_sites(),
            next_cycle: 1,
            cycles: Vec::new(),
            pending_allocation_site: RUNTIME_ALLOCATION_SITE_ID,
            allocation_profile: BTreeMap::new(),
        }
    }
}

impl GcDiagnosticsState {
    pub(crate) fn configure(
        &mut self,
        options: GcDiagnosticsOptions,
        allocation_sites: AllocationSiteRegistry,
    ) {
        self.options = options;
        self.allocation_sites = allocation_sites;
        self.next_cycle = 1;
        self.cycles.clear();
        self.pending_allocation_site = RUNTIME_ALLOCATION_SITE_ID;
        self.allocation_profile.clear();
    }

    pub(crate) fn set_pending_allocation_site(&mut self, site_id: u32) {
        if self.options.allocation_profile {
            self.pending_allocation_site = site_id;
        }
    }

    pub(crate) fn record_allocation(&mut self, size: usize, heap_type: u8, capacity: u32) {
        let site_id = std::mem::replace(
            &mut self.pending_allocation_site,
            RUNTIME_ALLOCATION_SITE_ID,
        );
        if !self.options.allocation_profile {
            return;
        }
        let key = AllocationProfileKey { site_id, heap_type };
        let bucket = self.allocation_profile.entry(key).or_default();
        bucket.count += 1;
        bucket.bytes += size as u64;
        bucket.capacity_total += capacity as u64;
    }

    pub(crate) fn record_host_allocation(&mut self, size: usize, heap_type: u8, capacity: u32) {
        if !self.options.allocation_profile {
            return;
        }
        let key = AllocationProfileKey {
            site_id: HOST_ALLOCATION_SITE_ID,
            heap_type,
        };
        let bucket = self.allocation_profile.entry(key).or_default();
        bucket.count += 1;
        bucket.bytes += size as u64;
        bucket.capacity_total += capacity as u64;
    }

    pub(crate) fn record_cycle(&mut self, trigger: &'static str, algorithm: &str, stats: GcStats) {
        if !self.options.trace {
            return;
        }
        let cycle = self.next_cycle;
        self.next_cycle += 1;
        self.cycles.push(GcCycleReport {
            cycle,
            step: 1,
            trigger: trigger.to_string(),
            algorithm: algorithm.to_string(),
            marked: stats.marked,
            swept: stats.swept,
            freed_bytes: stats.freed_bytes,
            elapsed: stats.elapsed,
        });
    }

    pub(crate) fn options(&self) -> GcDiagnosticsOptions {
        self.options
    }

    pub(crate) fn report(&self, heap_snapshot: Option<HeapSnapshot>) -> GcDiagnosticsReport {
        let mut summary = GcSummary::default();
        summary.cycles = self.cycles.len() as u64;
        for cycle in &self.cycles {
            summary.total_marked += cycle.marked;
            summary.total_swept += cycle.swept;
            summary.total_freed_bytes += cycle.freed_bytes;
            summary.total_elapsed += cycle.elapsed;
        }
        let allocation_profile = self
            .allocation_profile
            .iter()
            .map(|(key, bucket)| {
                let site = self.allocation_sites.describe(key.site_id, key.heap_type);
                AllocationProfileEntry {
                    site_id: site.site_id,
                    function_id: site.function_id,
                    function_name: site.function_name,
                    block: site.block,
                    instruction: site.instruction,
                    kind: if site.kind == "unknown" {
                        heap_type_name(key.heap_type).to_string()
                    } else {
                        site.kind
                    },
                    count: bucket.count,
                    bytes: bucket.bytes,
                    capacity_total: bucket.capacity_total,
                }
            })
            .collect();
        GcDiagnosticsReport {
            cycles: self.cycles.clone(),
            summary,
            heap_snapshot,
            allocation_profile,
        }
    }
}

pub(crate) fn capture_heap_snapshot(
    store: &mut Store<RuntimeState>,
    env: &WasmEnv,
) -> HeapSnapshot {
    let obj_table_ptr = read_global_i32(store, &env.obj_table_ptr).max(0) as usize;
    let obj_table_count = read_global_i32(store, &env.obj_table_count).max(0) as usize;
    let function_props_base = env
        .function_props_base
        .as_ref()
        .map(|g| read_global_i32(store, g).max(0) as u32)
        .unwrap_or(0);

    let (objects, edges) = {
        let data = env.memory.data(&*store);
        scan_heap_objects(data, obj_table_ptr, obj_table_count, function_props_base)
    };
    let total_bytes = objects.iter().map(|object| object.size).sum();
    let host_refs = collect_host_references(store.data(), function_props_base);
    HeapSnapshot {
        objects,
        edges,
        host_refs,
        total_bytes,
    }
}

fn read_global_i32(store: &mut Store<RuntimeState>, global: &wasmtime::Global) -> i32 {
    global.get(&mut *store).i32().unwrap_or(0)
}

fn scan_heap_objects(
    data: &[u8],
    obj_table_ptr: usize,
    obj_table_count: usize,
    function_props_base: u32,
) -> (Vec<HeapObjectSnapshot>, Vec<HeapEdgeSnapshot>) {
    let mut objects = Vec::new();
    let mut edges = Vec::new();
    for handle in 0..obj_table_count as u32 {
        let slot = obj_table_ptr + handle as usize * constants::HANDLE_TABLE_ENTRY_SIZE as usize;
        if slot + 4 > data.len() {
            break;
        }
        let ptr = read_u32_at(data, slot) as usize;
        if ptr == 0 || ptr + constants::HEAP_OBJECT_HEADER_SIZE as usize > data.len() {
            continue;
        }
        let Some(size) = object_size_from_memory(data, ptr) else {
            continue;
        };
        let heap_type = heap_type_from_memory(data, ptr).unwrap_or(u8::MAX);
        let capacity = match heap_type {
            wjsm_ir::HEAP_TYPE_ARRAY => {
                read_u32_at(data, ptr + constants::HEAP_ARRAY_CAPACITY_OFFSET as usize)
            }
            _ => read_u32_at(data, ptr + constants::HEAP_OBJECT_CAPACITY_OFFSET as usize),
        };
        let used_slots = match heap_type {
            wjsm_ir::HEAP_TYPE_ARRAY => {
                read_u32_at(data, ptr + constants::HEAP_ARRAY_LENGTH_OFFSET as usize)
            }
            _ => read_u32_at(
                data,
                ptr + constants::HEAP_OBJECT_PROPERTY_COUNT_OFFSET as usize,
            ),
        };
        objects.push(HeapObjectSnapshot {
            handle,
            ptr,
            type_name: heap_type_name(heap_type).to_string(),
            size,
            capacity,
            used_slots,
        });
        scan_object_edges(
            data,
            handle,
            ptr,
            heap_type,
            capacity,
            used_slots,
            function_props_base,
            &mut edges,
        );
    }
    (objects, edges)
}

fn scan_object_edges(
    data: &[u8],
    handle: u32,
    ptr: usize,
    heap_type: u8,
    capacity: u32,
    used_slots: u32,
    function_props_base: u32,
    edges: &mut Vec<HeapEdgeSnapshot>,
) {
    let proto = read_u32_at(data, ptr + constants::HEAP_OBJECT_PROTO_OFFSET as usize);
    if proto != u32::MAX {
        edges.push(HeapEdgeSnapshot {
            from: handle,
            label: "[[Prototype]]".to_string(),
            to: Some(proto),
            value_tag: "object".to_string(),
        });
    }

    match heap_type {
        wjsm_ir::HEAP_TYPE_ARRAY => {
            let limit = used_slots.min(capacity) as usize;
            for index in 0..limit {
                let offset = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + index * constants::HEAP_ARRAY_ELEMENT_SIZE as usize;
                let val = read_i64_at(data, offset);
                push_value_edge(
                    edges,
                    handle,
                    format!("element[{index}]"),
                    val,
                    function_props_base,
                );
            }
        }
        _ => {
            let limit = used_slots.min(capacity) as usize;
            for index in 0..limit {
                let slot = ptr
                    + constants::HEAP_OBJECT_HEADER_SIZE as usize
                    + index * constants::HEAP_OBJECT_PROPERTY_SLOT_SIZE as usize;
                let name_id =
                    read_u32_at(data, slot + constants::PROP_SLOT_NAME_ID_OFFSET as usize);
                let value_offset = slot + constants::PROP_SLOT_VALUE_OFFSET as usize;
                let getter_offset = slot + constants::PROP_SLOT_GETTER_OFFSET as usize;
                let setter_offset = slot + constants::PROP_SLOT_SETTER_OFFSET as usize;
                push_value_edge(
                    edges,
                    handle,
                    format!("property[{name_id}].value"),
                    read_i64_at(data, value_offset),
                    function_props_base,
                );
                push_value_edge(
                    edges,
                    handle,
                    format!("property[{name_id}].get"),
                    read_i64_at(data, getter_offset),
                    function_props_base,
                );
                push_value_edge(
                    edges,
                    handle,
                    format!("property[{name_id}].set"),
                    read_i64_at(data, setter_offset),
                    function_props_base,
                );
            }
        }
    }
}

fn push_value_edge(
    edges: &mut Vec<HeapEdgeSnapshot>,
    from: u32,
    label: String,
    val: i64,
    function_props_base: u32,
) {
    if !value::tag_needs_root(val) {
        return;
    }
    edges.push(HeapEdgeSnapshot {
        from,
        label,
        to: heap_target_handle_from_value(val, function_props_base),
        value_tag: value_tag_name(val).to_string(),
    });
}

fn collect_host_references(st: &RuntimeState, function_props_base: u32) -> Vec<HeapHostReference> {
    let mut refs = Vec::new();
    if let Ok(microtasks) = st.microtask_queue.lock() {
        for (idx, task) in microtasks.iter().enumerate() {
            match task {
                Microtask::PromiseReaction {
                    promise,
                    handler,
                    argument,
                    ..
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].promise"),
                        *promise,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].handler"),
                        *handler,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].argument"),
                        *argument,
                        function_props_base,
                    );
                }
                Microtask::PromiseResolveThenable {
                    promise,
                    thenable,
                    then,
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].promise"),
                        *promise,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].thenable"),
                        *thenable,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].then"),
                        *then,
                        function_props_base,
                    );
                }
                Microtask::MicrotaskCallback { callback } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].callback"),
                        *callback,
                        function_props_base,
                    );
                }
                Microtask::AsyncResume {
                    continuation,
                    resume_val,
                    ..
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].continuation"),
                        *continuation,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].resume_val"),
                        *resume_val,
                        function_props_base,
                    );
                }
                Microtask::CleanupFinalizationRegistry {
                    callback,
                    held_value,
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].callback"),
                        *callback,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].held_value"),
                        *held_value,
                        function_props_base,
                    );
                }
                Microtask::TransformStreamTransform {
                    callback,
                    this_val,
                    chunk,
                    controller,
                    write_promise,
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].callback"),
                        *callback,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].this_val"),
                        *this_val,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].chunk"),
                        *chunk,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].controller"),
                        *controller,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].write_promise"),
                        *write_promise,
                        function_props_base,
                    );
                }
                Microtask::TransformStreamFlush {
                    callback,
                    this_val,
                    controller,
                    close_promise,
                    ..
                } => {
                    if let Some(callback) = callback {
                        push_host_ref(
                            &mut refs,
                            format!("microtask_queue[{idx}].callback"),
                            *callback,
                            function_props_base,
                        );
                    }
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].this_val"),
                        *this_val,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].controller"),
                        *controller,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].close_promise"),
                        *close_promise,
                        function_props_base,
                    );
                }
                Microtask::ReadableStreamPull {
                    callback,
                    this_val,
                    controller,
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].callback"),
                        *callback,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].this_val"),
                        *this_val,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].controller"),
                        *controller,
                        function_props_base,
                    );
                }
                Microtask::WritableStreamSinkWrite {
                    callback,
                    this_val,
                    chunk,
                    controller,
                    write_promise,
                } => {
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].callback"),
                        *callback,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].this_val"),
                        *this_val,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].chunk"),
                        *chunk,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].controller"),
                        *controller,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].write_promise"),
                        *write_promise,
                        function_props_base,
                    );
                }
                Microtask::WritableStreamSinkClose {
                    callback,
                    this_val,
                    controller,
                    close_promise,
                    ..
                } => {
                    if let Some(callback) = callback {
                        push_host_ref(
                            &mut refs,
                            format!("microtask_queue[{idx}].callback"),
                            *callback,
                            function_props_base,
                        );
                    }
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].this_val"),
                        *this_val,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].controller"),
                        *controller,
                        function_props_base,
                    );
                    push_host_ref(
                        &mut refs,
                        format!("microtask_queue[{idx}].close_promise"),
                        *close_promise,
                        function_props_base,
                    );
                }
                Microtask::ReadableStreamPipeToPump { .. } => {}
            }
        }
    }

    if let Ok(promises) = st.promise_table.lock() {
        for (idx, entry) in promises.iter().enumerate() {
            if !entry.is_promise {
                continue;
            }
            match &entry.state {
                PromiseState::Fulfilled(v) => push_host_ref(
                    &mut refs,
                    format!("promise_table[{idx}].fulfilled"),
                    *v,
                    function_props_base,
                ),
                PromiseState::Rejected(v) => push_host_ref(
                    &mut refs,
                    format!("promise_table[{idx}].rejected"),
                    *v,
                    function_props_base,
                ),
                PromiseState::Pending => {}
            }
            for (reaction_idx, reaction) in entry
                .fulfill_reactions
                .iter()
                .chain(entry.reject_reactions.iter())
                .enumerate()
            {
                push_host_ref(
                    &mut refs,
                    format!("promise_table[{idx}].reaction[{reaction_idx}].target"),
                    reaction.target_promise,
                    function_props_base,
                );
                if let PromiseReactionKind::Normal { handler } = &reaction.kind {
                    push_host_ref(
                        &mut refs,
                        format!("promise_table[{idx}].reaction[{reaction_idx}].handler"),
                        *handler,
                        function_props_base,
                    );
                }
            }
        }
    }

    if let Ok(table) = st.continuation_table.lock() {
        for (idx, entry) in table.iter().enumerate() {
            push_host_ref(
                &mut refs,
                format!("continuation_table[{idx}].outer_promise"),
                entry.outer_promise,
                function_props_base,
            );
            for (var_idx, val) in entry.captured_vars.iter().enumerate() {
                push_host_ref(
                    &mut refs,
                    format!("continuation_table[{idx}].captured_vars[{var_idx}]"),
                    *val,
                    function_props_base,
                );
            }
        }
    }

    if let Ok(timers) = st.timers.lock() {
        for (idx, timer) in timers.iter().enumerate() {
            push_host_ref(
                &mut refs,
                format!("timers[{idx}].callback"),
                timer.callback,
                function_props_base,
            );
        }
    }

    crate::array_named_props::ArrayNamedPropsStore::trace_root_sources(
        &st.array_named_props,
        &mut |handle, name_id, val| {
            push_host_ref(
                &mut refs,
                format!("array_named_props[{handle}].property[{name_id}]"),
                val,
                function_props_base,
            );
        },
    );

    collect_linear_table_refs(
        &mut refs,
        "map_table",
        st.map_table.lock().ok().as_deref().map(|table| {
            table
                .iter()
                .flat_map(|entry| entry.keys.iter().chain(entry.values.iter()).copied())
                .collect::<Vec<_>>()
        }),
        function_props_base,
    );
    collect_linear_table_refs(
        &mut refs,
        "set_table",
        st.set_table.lock().ok().as_deref().map(|table| {
            table
                .iter()
                .flat_map(|entry| entry.values.iter().copied())
                .collect::<Vec<_>>()
        }),
        function_props_base,
    );
    collect_linear_table_refs(
        &mut refs,
        "finalization_registry_table",
        st.finalization_registry_table
            .lock()
            .ok()
            .as_deref()
            .map(|table| {
                let mut values = Vec::new();
                for entry in table.iter() {
                    values.push(entry.callback);
                    values.extend(
                        entry
                            .registrations
                            .iter()
                            .map(|registration| registration.held_value),
                    );
                }
                values
            }),
        function_props_base,
    );
    collect_linear_table_refs(
        &mut refs,
        "async_generator_table",
        st.async_generator_table
            .lock()
            .ok()
            .as_deref()
            .map(|table| {
                let mut values = Vec::new();
                for entry in table.iter() {
                    values.push(entry.continuation);
                    values.extend(entry.waiting_resume_promise);
                    if let Some(req) = &entry.active_request {
                        values.extend([req.value, req.promise]);
                    }
                    for req in entry.queue.iter() {
                        values.extend([req.value, req.promise]);
                    }
                }
                values
            }),
        function_props_base,
    );
    collect_linear_table_refs(
        &mut refs,
        "generator_table",
        st.generator_table.lock().ok().as_deref().map(|table| {
            table
                .iter()
                .map(|entry| entry.continuation)
                .collect::<Vec<_>>()
        }),
        function_props_base,
    );
    collect_linear_table_refs(
        &mut refs,
        "async_from_sync_iterators",
        st.async_from_sync_iterators
            .lock()
            .ok()
            .as_deref()
            .map(|table| {
                table
                    .iter()
                    .flat_map(|entry| [entry.sync_iterator, entry.outer_iter])
                    .collect::<Vec<_>>()
            }),
        function_props_base,
    );

    collect_host_side_table_refs(
        &mut refs,
        "readable_stream_table",
        &st.readable_stream_table,
        function_props_base,
        |entry| {
            let mut values = Vec::new();
            values.extend(entry.response_body_object);
            if let Some(pipe_to) = entry.pipe_to {
                values.push(pipe_to.promise);
            }
            values
        },
    );
    collect_host_side_table_refs(
        &mut refs,
        "reader_table",
        &st.reader_table,
        function_props_base,
        |entry| {
            [
                entry.pending_read_promise,
                entry.pending_byob_view,
                entry.closed_promise,
            ]
            .into_iter()
            .flatten()
            .collect()
        },
    );
    collect_host_side_table_refs(
        &mut refs,
        "byob_request_table",
        &st.byob_request_table,
        function_props_base,
        |entry| vec![entry.view, entry.promise],
    );
    collect_host_side_table_refs(
        &mut refs,
        "stream_controller_table",
        &st.stream_controller_table,
        function_props_base,
        |entry| {
            let mut values = Vec::new();
            values.extend(entry.underlying_source);
            values.extend(entry.pull_callback);
            values.extend(entry.cancel_callback);
            values.extend(entry.write_callback);
            values.extend(entry.sink_close_callback);
            values.extend(entry.strategy_size);
            values.extend(entry.abort_reason);
            values.extend(entry.chunk_queue.iter().copied());
            values
        },
    );
    collect_host_side_table_refs(
        &mut refs,
        "writable_stream_table",
        &st.writable_stream_table,
        function_props_base,
        |entry| {
            [entry.error, entry.abort_signal]
                .into_iter()
                .flatten()
                .collect()
        },
    );
    collect_host_side_table_refs(
        &mut refs,
        "writer_table",
        &st.writer_table,
        function_props_base,
        |entry| {
            [entry.closed_promise, entry.ready_promise]
                .into_iter()
                .flatten()
                .collect()
        },
    );
    collect_host_side_table_refs(
        &mut refs,
        "transform_stream_table",
        &st.transform_stream_table,
        function_props_base,
        |entry| {
            [
                entry.transform_callback,
                entry.flush_callback,
                entry.transformer_this,
                entry.readable_obj,
                entry.writable_obj,
            ]
            .into_iter()
            .flatten()
            .collect()
        },
    );

    refs
}

fn collect_linear_table_refs(
    refs: &mut Vec<HeapHostReference>,
    table: &str,
    values: Option<Vec<i64>>,
    function_props_base: u32,
) {
    if let Some(values) = values {
        for (idx, val) in values.into_iter().enumerate() {
            push_host_ref(refs, format!("{table}[{idx}]"), val, function_props_base);
        }
    }
}

fn collect_host_side_table_refs<T>(
    refs: &mut Vec<HeapHostReference>,
    table_name: &str,
    table: &crate::HostSideTable<T>,
    function_props_base: u32,
    values: impl Fn(&T) -> Vec<i64>,
) {
    if let Ok(inner) = table.inner.lock() {
        for (idx, entry) in inner.entries.iter().enumerate() {
            let Some(entry) = entry else {
                continue;
            };
            for (value_idx, val) in values(entry).into_iter().enumerate() {
                push_host_ref(
                    refs,
                    format!("{table_name}[{idx}].value[{value_idx}]"),
                    val,
                    function_props_base,
                );
            }
        }
    }
}

fn push_host_ref(
    refs: &mut Vec<HeapHostReference>,
    source: String,
    val: i64,
    function_props_base: u32,
) {
    if !value::tag_needs_root(val) {
        return;
    }
    refs.push(HeapHostReference {
        source,
        to: heap_target_handle_from_value(val, function_props_base),
        value_tag: value_tag_name(val).to_string(),
    });
}

fn heap_target_handle_from_value(val: i64, function_props_base: u32) -> Option<u32> {
    if value::is_object(val) || value::is_array(val) {
        Some(value::decode_handle(val))
    } else if value::is_function(val) {
        Some(function_props_base.saturating_add(value::decode_function_idx(val)))
    } else {
        None
    }
}

fn heap_type_name(heap_type: u8) -> &'static str {
    match heap_type {
        wjsm_ir::HEAP_TYPE_OBJECT => "object",
        wjsm_ir::HEAP_TYPE_ARRAY => "array",
        wjsm_ir::HEAP_TYPE_PROMISE => "promise",
        wjsm_ir::HEAP_TYPE_CONTINUATION => "continuation",
        wjsm_ir::HEAP_TYPE_ASYNC_GENERATOR => "async-generator",
        wjsm_ir::HEAP_TYPE_ARGUMENTS => "arguments",
        _ => "unknown",
    }
}

fn value_tag_name(val: i64) -> &'static str {
    if value::is_object(val) {
        "object"
    } else if value::is_array(val) {
        "array"
    } else if value::is_function(val) {
        "function"
    } else if value::is_closure(val) {
        "closure"
    } else if value::is_bound(val) {
        "bound"
    } else if value::is_proxy(val) {
        "proxy"
    } else if value::is_native_callable(val) {
        "native-callable"
    } else if value::is_bigint(val) {
        "bigint"
    } else if value::is_symbol(val) {
        "symbol"
    } else if value::is_regexp(val) {
        "regexp"
    } else if value::is_scope_record(val) {
        "scope-record"
    } else if value::is_iterator(val) {
        "iterator"
    } else if value::is_enumerator(val) {
        "enumerator"
    } else if value::is_exception(val) {
        "exception"
    } else if value::is_runtime_string_handle(val) {
        "runtime-string"
    } else {
        "scalar"
    }
}

fn allocation_site_kind_name(kind: u8) -> &'static str {
    match kind {
        1 => "object",
        2 => "array",
        _ => "unknown",
    }
}

fn decode_optional_u32(value: u32) -> Option<u32> {
    (value != u32::MAX).then_some(value)
}

fn read_u32_at(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_i64_at(data: &[u8], offset: usize) -> i64 {
    if offset + 8 > data.len() {
        return value::encode_undefined();
    }
    i64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

struct SectionCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SectionCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let bytes = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(bytes)
    }

    fn read_u8(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn read_u32(&mut self) -> Option<u32> {
        let bytes = self.read_bytes(4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_string(&mut self) -> Option<String> {
        let len = self.read_u32()? as usize;
        let bytes = self.read_bytes(len)?;
        std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
    }
}

pub fn format_gc_diagnostics_report(report: &GcDiagnosticsReport) -> String {
    let mut out = String::new();
    if !report.cycles.is_empty() {
        out.push_str("GC trace:\n");
        for cycle in &report.cycles {
            let _ = writeln!(
                out,
                "cycle={} step={} trigger={} algorithm={} marked={} swept={} freed_bytes={} elapsed_us={}",
                cycle.cycle,
                cycle.step,
                cycle.trigger,
                cycle.algorithm,
                cycle.marked,
                cycle.swept,
                cycle.freed_bytes,
                cycle.elapsed.as_micros()
            );
        }
    }

    if report.summary.cycles > 0
        || report.heap_snapshot.is_some()
        || !report.allocation_profile.is_empty()
    {
        out.push_str("GC stats:\n");
        let _ = writeln!(
            out,
            "cycles={} total_marked={} total_swept={} total_freed_bytes={} total_elapsed_us={}",
            report.summary.cycles,
            report.summary.total_marked,
            report.summary.total_swept,
            report.summary.total_freed_bytes,
            report.summary.total_elapsed.as_micros()
        );
    }

    if let Some(snapshot) = &report.heap_snapshot {
        out.push_str("Heap snapshot:\n");
        let _ = writeln!(
            out,
            "objects={} edges={} bytes={} host_refs={}",
            snapshot.objects.len(),
            snapshot.edges.len(),
            snapshot.total_bytes,
            snapshot.host_refs.len()
        );
        for object in &snapshot.objects {
            let _ = writeln!(
                out,
                "object handle={} type={} ptr={} size={} capacity={} used_slots={}",
                object.handle,
                object.type_name,
                object.ptr,
                object.size,
                object.capacity,
                object.used_slots
            );
        }
        for edge in &snapshot.edges {
            let target = edge
                .to
                .map_or_else(|| "none".to_string(), |to| to.to_string());
            let _ = writeln!(
                out,
                "edge from={} label={} to={} value_tag={}",
                edge.from, edge.label, target, edge.value_tag
            );
        }
        for host_ref in &snapshot.host_refs {
            let target = host_ref
                .to
                .map_or_else(|| "none".to_string(), |to| to.to_string());
            let _ = writeln!(
                out,
                "host_ref source={} to={} value_tag={}",
                host_ref.source, target, host_ref.value_tag
            );
        }
    }

    if !report.allocation_profile.is_empty() {
        out.push_str("Allocation profile:\n");
        for entry in &report.allocation_profile {
            let function_id = entry
                .function_id
                .map_or_else(|| "none".to_string(), |id| id.to_string());
            let block = entry
                .block
                .map_or_else(|| "none".to_string(), |id| id.to_string());
            let instruction = entry
                .instruction
                .map_or_else(|| "none".to_string(), |id| id.to_string());
            let avg_size = if entry.count == 0 {
                0
            } else {
                entry.bytes / entry.count
            };
            let _ = writeln!(
                out,
                "site={} function_id={} function={} block={} instruction={} kind={} count={} bytes={} avg_size={} capacity_total={}",
                entry.site_id,
                function_id,
                entry.function_name,
                block,
                instruction,
                entry.kind,
                entry.count,
                entry.bytes,
                avg_size,
                entry.capacity_total
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_site_section_roundtrips() {
        let mut data = Vec::new();
        data.extend_from_slice(ALLOCATION_SITES_MAGIC);
        data.extend_from_slice(&ALLOCATION_SITES_VERSION.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&7u32.to_le_bytes());
        data.push(1);
        data.extend_from_slice(&5u32.to_le_bytes());
        data.extend_from_slice(b"@main");

        let mut registry = AllocationSiteRegistry::with_builtin_sites();
        registry.merge_section(&data);
        let site = registry.describe(2, wjsm_ir::HEAP_TYPE_OBJECT);
        assert_eq!(site.function_id, Some(0));
        assert_eq!(site.function_name, "@main");
        assert_eq!(site.block, Some(3));
        assert_eq!(site.instruction, Some(7));
        assert_eq!(site.kind, "object");
    }

    #[test]
    fn formatter_is_stable_for_gc_report() {
        let report = GcDiagnosticsReport {
            cycles: vec![GcCycleReport {
                cycle: 1,
                step: 1,
                trigger: "proactive".to_string(),
                algorithm: "mark-sweep".to_string(),
                marked: 4,
                swept: 3,
                freed_bytes: 96,
                elapsed: Duration::from_micros(12),
            }],
            summary: GcSummary {
                cycles: 1,
                total_marked: 4,
                total_swept: 3,
                total_freed_bytes: 96,
                total_elapsed: Duration::from_micros(12),
            },
            heap_snapshot: Some(HeapSnapshot {
                objects: vec![HeapObjectSnapshot {
                    handle: 0,
                    ptr: 1024,
                    type_name: "object".to_string(),
                    size: 48,
                    capacity: 1,
                    used_slots: 1,
                }],
                edges: vec![HeapEdgeSnapshot {
                    from: 0,
                    label: "property[224].value".to_string(),
                    to: Some(1),
                    value_tag: "array".to_string(),
                }],
                host_refs: vec![HeapHostReference {
                    source: "promise_table[0].fulfilled".to_string(),
                    to: Some(0),
                    value_tag: "object".to_string(),
                }],
                total_bytes: 48,
            }),
            allocation_profile: vec![AllocationProfileEntry {
                site_id: 2,
                function_id: Some(0),
                function_name: "@main".to_string(),
                block: Some(0),
                instruction: Some(1),
                kind: "object".to_string(),
                count: 2,
                bytes: 96,
                capacity_total: 2,
            }],
        };

        assert_eq!(
            format_gc_diagnostics_report(&report),
            "GC trace:\n\
cycle=1 step=1 trigger=proactive algorithm=mark-sweep marked=4 swept=3 freed_bytes=96 elapsed_us=12\n\
GC stats:\n\
cycles=1 total_marked=4 total_swept=3 total_freed_bytes=96 total_elapsed_us=12\n\
Heap snapshot:\n\
objects=1 edges=1 bytes=48 host_refs=1\n\
object handle=0 type=object ptr=1024 size=48 capacity=1 used_slots=1\n\
edge from=0 label=property[224].value to=1 value_tag=array\n\
host_ref source=promise_table[0].fulfilled to=0 value_tag=object\n\
Allocation profile:\n\
site=2 function_id=0 function=@main block=0 instruction=1 kind=object count=2 bytes=96 avg_size=48 capacity_total=2\n"
        );
    }
}
