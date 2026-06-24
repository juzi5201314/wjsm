//! SharedArrayBuffer backing state、grow 元数据与 agent cluster waiter store 的单一 owner。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use tokio::time::Instant;
use wasmtime::Caller;
use wjsm_ir::{constants, value};

use crate::runtime_heap::alloc_host_object;
use crate::runtime_heap::define_host_data_property_from_caller;
use crate::runtime_promises::set_runtime_error;
use crate::runtime_values::{
    find_property_slot_by_name_id, read_object_property_by_name, resolve_handle,
    resolve_handle_idx, write_object_property_by_name_id,
};
use crate::{RuntimeState, WasmEnv};

#[derive(Clone, Debug)]
pub(crate) struct SharedArrayBufferEntry {
    pub(crate) data: Arc<RwLock<Vec<u8>>>,
    pub(crate) byte_length: u64,
    pub(crate) max_byte_length: Option<u64>,
}

impl SharedArrayBufferEntry {
    pub(crate) fn growable(&self) -> bool {
        self.max_byte_length.is_some()
    }

    pub(crate) fn max_byte_length(&self) -> u64 {
        self.max_byte_length.unwrap_or(self.byte_length)
    }
}

pub struct SharedRuntimeState {
    pub(crate) sab_table: Arc<Mutex<Vec<SharedArrayBufferEntry>>>,
    pub(crate) waiter_lists: Arc<Mutex<HashMap<(u32, u32), WaiterList>>>,
    pub(crate) agent_state: Arc<AgentState>,
}

#[derive(Default)]
pub(crate) struct AgentBroadcastSlot {
    pub(crate) sab_handle: Option<u32>,
    pub(crate) lock: u8,
}

pub(crate) struct AgentState {
    pub(crate) reports: Arc<Mutex<Vec<String>>>,
    pub(crate) broadcast_slot: Mutex<AgentBroadcastSlot>,
    pub(crate) broadcast_cv: Condvar,
    pub(crate) broadcast_callback_done: Mutex<bool>,
    pub(crate) broadcast_callback_done_cv: Condvar,
    pub(crate) next_agent_id: AtomicU32,
}

pub(crate) struct WaiterList {
    pub(crate) waiters: VecDeque<WaiterRecord>,
}

pub(crate) struct WaiterRecord {
    pub(crate) notified: Arc<AtomicBool>,
    pub(crate) condvar: Arc<Condvar>,
    pub(crate) signal: Arc<tokio::sync::Notify>,
    #[allow(dead_code)]
    pub(crate) deadline: Option<Instant>,
    pub(crate) promise: Option<i64>,
}
#[derive(Clone)]
pub(crate) struct WaiterHandle {
    pub(crate) notified: Arc<AtomicBool>,
    pub(crate) signal: Arc<tokio::sync::Notify>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufferBacking {
    ArrayBuffer {
        handle: u32,
        byte_length: u32,
    },
    SharedArrayBuffer {
        handle: u32,
        byte_length: u32,
        growable: bool,
    },
}

pub(crate) fn new_shared_runtime_state() -> Arc<SharedRuntimeState> {
    Arc::new(SharedRuntimeState {
        sab_table: Arc::new(Mutex::new(Vec::new())),
        waiter_lists: Arc::new(Mutex::new(HashMap::new())),
        agent_state: Arc::new(AgentState {
            reports: Arc::new(Mutex::new(Vec::new())),
            broadcast_slot: Mutex::new(AgentBroadcastSlot::default()),
            broadcast_cv: Condvar::new(),
            broadcast_callback_done: Mutex::new(true),
            broadcast_callback_done_cv: Condvar::new(),
            next_agent_id: AtomicU32::new(0),
        }),
    })
}

const SAB_HANDLE_PROP: &str = "__sharedarraybuffer_handle__";

/// ToIndex 语义：将 JS 值转为非负整数索引；失败时写入 runtime_error。
pub(crate) fn to_index_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    error_message: &'static str,
) -> Option<u64> {
    if value::is_undefined(value_raw) {
        return Some(0);
    }
    let number = if value::is_f64(value_raw) {
        value::decode_f64(value_raw)
    } else if value::is_bool(value_raw) {
        if value::decode_bool(value_raw) {
            1.0
        } else {
            0.0
        }
    } else {
        set_runtime_error(caller.data(), error_message.to_string());
        return None;
    };

    if !number.is_finite() || number < 0.0 {
        set_runtime_error(caller.data(), error_message.to_string());
        return None;
    }
    let truncated = number.trunc();
    if truncated > u64::MAX as f64 {
        set_runtime_error(caller.data(), error_message.to_string());
        return None;
    }
    Some(truncated as u64)
}

pub(crate) fn read_sab_handle_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
) -> Option<u32> {
    if !value::is_object(obj) {
        return None;
    }
    let obj_ptr = resolve_handle(caller, obj)?;
    let h = read_object_property_by_name(caller, obj_ptr, SAB_HANDLE_PROP)?;
    if !value::is_f64(h) {
        return None;
    }
    Some(value::decode_f64(h) as u32)
}

fn read_sab_handle_from_this(caller: &mut Caller<'_, RuntimeState>, this_val: i64) -> Option<u32> {
    read_sab_handle_from_object(caller, this_val)
}

fn sab_entry_for_this(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> Option<SharedArrayBufferEntry> {
    let handle = read_sab_handle_from_this(caller, this_val)?;
    let shared = caller.data().shared_state.as_ref()?.clone();
    let table = shared.sab_table.lock().ok()?;
    table.get(handle as usize).cloned()
}

fn set_sab_host_data_property(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    val: i64,
) {
    let Some(obj_ptr) = resolve_handle(caller, obj) else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
        return;
    };
    let Some(name_id) = crate::find_memory_c_string(caller, name) else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
        return;
    };
    let flags =
        constants::FLAG_CONFIGURABLE | constants::FLAG_ENUMERABLE | constants::FLAG_WRITABLE;
    if find_property_slot_by_name_id(caller, obj_ptr, name_id).is_some() {
        write_object_property_by_name_id(caller, obj_ptr, obj, name_id, val, flags);
    } else {
        let _ = define_host_data_property_from_caller(caller, obj, name, val);
    }
}

fn define_sab_data_properties(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    handle: u32,
    byte_length: u64,
    growable: bool,
    max_byte_length: u64,
) {
    set_sab_host_data_property(
        caller,
        obj,
        SAB_HANDLE_PROP,
        value::encode_f64(handle as f64),
    );
    set_sab_host_data_property(
        caller,
        obj,
        "byteLength",
        value::encode_f64(byte_length as f64),
    );
    set_sab_host_data_property(caller, obj, "growable", value::encode_bool(growable));
    set_sab_host_data_property(
        caller,
        obj,
        "maxByteLength",
        value::encode_f64(max_byte_length as f64),
    );
}

/// 将已有 `sab_table` 条目包装为 JS 可见的 SharedArrayBuffer 对象（不新建 backing）。
pub(crate) fn materialize_shared_array_buffer_by_handle(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    shared: &Arc<SharedRuntimeState>,
    handle: u32,
) -> i64 {
    let table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get(handle as usize) else {
        return value::encode_undefined();
    };
    let byte_length = entry.byte_length;
    let growable = entry.growable();
    let max_observable = entry.max_byte_length();
    drop(table);
    let obj = alloc_host_object(caller, env, 4);
    define_sab_data_properties(caller, obj, handle, byte_length, growable, max_observable);
    obj
}

pub(crate) fn construct_shared_array_buffer(
    caller: &mut Caller<'_, RuntimeState>,
    length: i64,
    options: i64,
    target_obj: i64,
) -> i64 {
    let byte_length =
        match to_index_from_value(caller, length, "RangeError: Invalid array buffer length") {
            Some(v) => v,
            None => return value::encode_undefined(),
        };

    let mut max_byte_length: Option<u64> = None;
    if !value::is_undefined(options) && !value::is_null(options) {
        if !value::is_object(options) {
            set_runtime_error(
                caller.data(),
                "TypeError: SharedArrayBuffer options must be an object".to_string(),
            );
            return value::encode_undefined();
        }
        let opt_ptr =
            match resolve_handle_idx(caller, value::decode_object_handle(options) as usize) {
                Some(p) => p,
                None => return value::encode_undefined(),
            };
        if let Some(mbl_raw) = read_object_property_by_name(caller, opt_ptr, "maxByteLength") {
            let mbl =
                match to_index_from_value(caller, mbl_raw, "RangeError: Invalid maxByteLength") {
                    Some(v) => v,
                    None => return value::encode_undefined(),
                };
            if mbl < byte_length {
                set_runtime_error(
                    caller.data(),
                    "RangeError: maxByteLength must not be less than byte length".to_string(),
                );
                return value::encode_undefined();
            }
            max_byte_length = Some(mbl);
        }
    }

    let shared = match caller.data().shared_state.clone() {
        Some(s) => s,
        None => return value::encode_undefined(),
    };

    let entry = SharedArrayBufferEntry {
        data: Arc::new(RwLock::new(vec![0u8; byte_length as usize])),
        byte_length,
        max_byte_length,
    };
    let growable = entry.growable();
    let max_observable = entry.max_byte_length();

    let handle = {
        let mut table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
        table.push(entry);
        (table.len() - 1) as u32
    };

    let obj = if value::is_object(target_obj) {
        target_obj
    } else {
        let env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &env, 4)
    };

    define_sab_data_properties(caller, obj, handle, byte_length, growable, max_observable);
    obj
}

pub(crate) fn shared_array_buffer_byte_length(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    match sab_entry_for_this(caller, this_val) {
        Some(entry) => value::encode_f64(entry.byte_length as f64),
        None => value::encode_undefined(),
    }
}

pub(crate) fn shared_array_buffer_growable(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    match sab_entry_for_this(caller, this_val) {
        Some(entry) => value::encode_bool(entry.growable()),
        None => value::encode_undefined(),
    }
}

pub(crate) fn shared_array_buffer_max_byte_length(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    match sab_entry_for_this(caller, this_val) {
        Some(entry) => value::encode_f64(entry.max_byte_length() as f64),
        None => value::encode_undefined(),
    }
}

/// SharedArrayBuffer.prototype.grow：仅 growable SAB 可扩容，新区域零填充。
pub(crate) fn shared_array_buffer_grow(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    new_length_raw: i64,
) -> i64 {
    let Some(handle) = read_sab_handle_from_this(caller, this_val) else {
        set_runtime_error(
            caller.data(),
            "TypeError: SharedArrayBuffer.prototype.grow called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    };
    let new_length = match to_index_from_value(
        caller,
        new_length_raw,
        "RangeError: Invalid array buffer length",
    ) {
        Some(v) => v,
        None => return value::encode_undefined(),
    };
    let shared = match caller.data().shared_state.clone() {
        Some(s) => s,
        None => return value::encode_undefined(),
    };
    let mut table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.get_mut(handle as usize) else {
        set_runtime_error(
            caller.data(),
            "TypeError: SharedArrayBuffer.prototype.grow called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    };
    if entry.max_byte_length.is_none() {
        set_runtime_error(
            caller.data(),
            "TypeError: SharedArrayBuffer.prototype.grow can only be used with growable SharedArrayBuffers"
                .to_string(),
        );
        return value::encode_undefined();
    }
    let max_len = entry.max_byte_length();
    if new_length < entry.byte_length {
        set_runtime_error(
            caller.data(),
            "RangeError: new length is smaller than the current length".to_string(),
        );
        return value::encode_undefined();
    }
    if new_length > max_len {
        set_runtime_error(
            caller.data(),
            "RangeError: new length exceeds maxByteLength".to_string(),
        );
        return value::encode_undefined();
    }
    if new_length > entry.byte_length {
        let mut data = entry.data.write().expect("sab data lock");
        data.resize(new_length as usize, 0);
        entry.byte_length = new_length;
    }
    let growable = entry.growable();
    let max_observable = entry.max_byte_length();
    drop(table);
    define_sab_data_properties(
        caller,
        this_val,
        handle,
        new_length,
        growable,
        max_observable,
    );
    value::encode_f64(new_length as f64)
}

/// SharedArrayBuffer.prototype.slice：复制区间到新的 fixed-length SAB。
pub(crate) fn shared_array_buffer_slice(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    begin_raw: i64,
    end_raw: i64,
) -> i64 {
    let Some(entry) = sab_entry_for_this(caller, this_val) else {
        set_runtime_error(
            caller.data(),
            "TypeError: SharedArrayBuffer.prototype.slice called on incompatible receiver"
                .to_string(),
        );
        return value::encode_undefined();
    };
    let byte_len = entry.byte_length;
    let begin = to_index_from_value(caller, begin_raw, "RangeError: Invalid begin index")
        .unwrap_or(0)
        .min(byte_len);
    let end = if value::is_undefined(end_raw) {
        byte_len
    } else {
        match to_index_from_value(caller, end_raw, "RangeError: Invalid end index") {
            Some(v) => v.min(byte_len),
            None => return value::encode_undefined(),
        }
    };
    let start = begin.min(end);
    let stop = begin.max(end);
    let new_len = stop.saturating_sub(start);
    let slice_bytes = {
        let data = entry.data.read().expect("sab data read");
        data[start as usize..stop as usize].to_vec()
    };
    let shared = match caller.data().shared_state.clone() {
        Some(s) => s,
        None => return value::encode_undefined(),
    };
    let new_entry = SharedArrayBufferEntry {
        data: Arc::new(RwLock::new(slice_bytes)),
        byte_length: new_len,
        max_byte_length: None,
    };
    let new_handle = {
        let mut table = shared.sab_table.lock().unwrap_or_else(|e| e.into_inner());
        table.push(new_entry);
        (table.len() - 1) as u32
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 4);
    define_sab_data_properties(caller, obj, new_handle, new_len, false, new_len);
    obj
}

/// SharedArrayBuffer[Symbol.species]：返回 this 上的构造函数。
pub(crate) fn shared_array_buffer_species(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
) -> i64 {
    let _ = caller;
    this_val
}

/// Atomics.pause：规范 hint，无操作。
pub(crate) fn atomics_pause(_caller: &mut Caller<'_, RuntimeState>) -> i64 {
    value::encode_undefined()
}
/// 入队 waiter（用于 wait / waitAsync）。返回 notified flag 用于唤醒检测。
/// key = (sab_handle, byte_offset)
pub(crate) fn enter_waiter(
    shared: &SharedRuntimeState,
    sab_handle: u32,
    byte_offset: u32,
    deadline: Option<Instant>,
    promise: Option<i64>,
) -> WaiterHandle {
    let notified = Arc::new(AtomicBool::new(false));
    let condvar = Arc::new(Condvar::new());
    let signal = Arc::new(tokio::sync::Notify::new());
    let rec = WaiterRecord {
        notified: Arc::clone(&notified),
        condvar,
        signal: Arc::clone(&signal),
        deadline,
        promise,
    };
    let mut lists = shared.waiter_lists.lock().unwrap_or_else(|e| e.into_inner());
    let list = lists
        .entry((sab_handle, byte_offset))
        .or_insert_with(|| WaiterList {
            waiters: VecDeque::new(),
        });
    list.waiters.push_back(rec);
    WaiterHandle { notified, signal }
}
/// 按 FIFO 唤醒最多 count 个 waiter，返回实际唤醒数。设置 notified 并可触发 microtask 结算 promise。
/// 按 FIFO 唤醒最多 count 个 waiter，返回实际唤醒数 + 被唤醒的 promise 句柄列表（用于 async wait 结算）。
pub(crate) fn notify_waiters_with_promises(
    shared: &SharedRuntimeState,
    sab_handle: u32,
    byte_offset: u32,
    count: u32,
) -> (u32, Vec<i64>) {
    let mut lists = shared.waiter_lists.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(list) = lists.get_mut(&(sab_handle, byte_offset)) {
        let mut woken = 0u32;
        let mut ps = Vec::new();
        for _ in 0..count {
            if let Some(rec) = list.waiters.pop_front() {
                rec.notified
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                rec.condvar.notify_one();
                rec.signal.notify_one();
                if let Some(p) = rec.promise {
                    ps.push(p);
                }
                woken += 1;
            } else {
                break;
            }
        }
        (woken, ps)
    } else {
        (0, vec![])
    }
}
/// 超时移除 waiter（由 async wait future 调用）。
pub(crate) fn remove_waiter(
    shared: &SharedRuntimeState,
    sab_handle: u32,
    byte_offset: u32,
    notified: &Arc<AtomicBool>,
) {
    let mut lists = shared.waiter_lists.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(list) = lists.get_mut(&(sab_handle, byte_offset)) {
        list.waiters.retain(|w| !Arc::ptr_eq(&w.notified, notified));
    }
}
/// 解析 ArrayBuffer / SharedArrayBuffer 对象，返回 backing 信息（handle 索引对应 table，is_shared 由 variant 区分）。
/// 用于 DataView / TypedArray 构造时决定 buffer_handle 指向 ab_table 还是 sab_table，并设置 entry.is_shared 。
pub(crate) fn resolve_buffer_backing(
    caller: &mut Caller<'_, RuntimeState>,
    buffer: i64,
) -> Option<BufferBacking> {
    if !value::is_object(buffer) {
        return None;
    }
    let ptr = resolve_handle(caller, buffer)?;
    let sab_h = read_object_property_by_name(caller, ptr, "__sharedarraybuffer_handle__");
    let ab_h = read_object_property_by_name(caller, ptr, "__arraybuffer_handle__");
    let bl = read_object_property_by_name(caller, ptr, "byteLength")
        .and_then(|v| {
            if value::is_f64(v) {
                Some(value::decode_f64(v) as u32)
            } else {
                None
            }
        })
        .unwrap_or(0);
    if let Some(hv) = sab_h
        && value::is_f64(hv)
    {
        let growable = read_object_property_by_name(caller, ptr, "maxByteLength").is_some();
        return Some(BufferBacking::SharedArrayBuffer {
            handle: value::decode_f64(hv) as u32,
            byte_length: bl,
            growable,
        });
    }
    if let Some(hv) = ab_h
        && value::is_f64(hv)
    {
        return Some(BufferBacking::ArrayBuffer {
            handle: value::decode_f64(hv) as u32,
            byte_length: bl,
        });
    }
    None
}
/// DataView 字节读取（根据 is_shared 选择 sab_table 或 arraybuffer_table）。
pub(crate) fn dataview_read_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: u32,
    is_shared: bool,
    abs_offset: usize,
    out: &mut [u8],
) -> bool {
    if is_shared {
        let Some(shared) = caller.data().shared_state.as_ref().cloned() else {
            return false;
        };
        let Ok(table) = shared.sab_table.lock() else {
            return false;
        };
        let Some(entry) = table.get(buf_handle as usize) else {
            return false;
        };
        let Ok(data) = entry.data.read() else {
            return false;
        };
        if abs_offset + out.len() > data.len() {
            return false;
        }
        out.copy_from_slice(&data[abs_offset..abs_offset + out.len()]);
        true
    } else {
        let Ok(table) = caller.data().arraybuffer_table.lock() else {
            return false;
        };
        let Some(entry) = table.get(buf_handle as usize) else {
            return false;
        };
        if abs_offset + out.len() > entry.data.len() {
            return false;
        }
        out.copy_from_slice(&entry.data[abs_offset..abs_offset + out.len()]);
        true
    }
}
/// DataView 字节写入。
pub(crate) fn dataview_set_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    buf_handle: u32,
    is_shared: bool,
    abs_offset: usize,
    bytes: &[u8],
) -> bool {
    if is_shared {
        let shared = match caller.data().shared_state.as_ref().cloned() {
            Some(s) => s,
            None => return false,
        };
        let table = match shared.sab_table.lock() {
            Ok(t) => t,
            Err(_) => return false,
        };
        let entry = match table.get(buf_handle as usize) {
            Some(e) => e,
            None => return false,
        };
        let mut data = match entry.data.write() {
            Ok(d) => d,
            Err(_) => return false,
        };
        if abs_offset + bytes.len() > data.len() {
            return false;
        }
        data[abs_offset..abs_offset + bytes.len()].copy_from_slice(bytes);
        true
    } else {
        let mut table = match caller.data().arraybuffer_table.lock() {
            Ok(t) => t,
            Err(_) => return false,
        };
        let entry = match table.get_mut(buf_handle as usize) {
            Some(e) => e,
            None => return false,
        };
        if abs_offset + bytes.len() > entry.data.len() {
            return false;
        }
        entry.data[abs_offset..abs_offset + bytes.len()].copy_from_slice(bytes);
        true
    }
}
