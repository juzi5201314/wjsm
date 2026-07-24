//! Worker / postMessage 跨 Store 结构化克隆：可 Send 的序列化形式 + transfer。
//!
//! 序列化在源 agent（Caller）上完成；反序列化在目标 agent 的 Store/Caller 上重建。
//! `structuredClone(value, { transfer })` 复用同一套路径（同 Store 内 ser → de）。

mod deserialize;
mod serialize;

pub(crate) use deserialize::deserialize_value;
#[allow(unused_imports)] // Caller 便捷路径；当前 worker drain 直接用 deserialize_value。
pub(crate) use deserialize::deserialize_value_from_caller;
pub(crate) use serialize::{
    parse_transfer_list, serialize_for_post_message, serialize_value, transfer_arg_from_options,
};

pub(crate) const MESSAGE_PORT_ID_PROP: &str = "__message_port_id__";
pub(crate) const SAB_HANDLE_PROP: &str = "__sharedarraybuffer_handle__";

/// 跨 Store 可传递的结构化克隆载荷（`Send + Clone`）。
#[derive(Clone, Debug)]
pub enum SerializedValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    /// 十进制文本，避免依赖 `num_bigint` 在边界上的字节序约定。
    BigInt(String),
    /// `id` 为序列化图中的首次出现编号，供 `Ref` 回指（处理环）。
    Array {
        id: usize,
        items: Vec<SerializedValue>,
    },
    Object {
        id: usize,
        entries: Vec<(String, SerializedValue)>,
    },
    Map {
        id: usize,
        entries: Vec<(SerializedValue, SerializedValue)>,
    },
    Set {
        id: usize,
        values: Vec<SerializedValue>,
    },
    Date {
        id: usize,
        ms: f64,
    },
    RegExp {
        id: usize,
        source: String,
        flags: String,
    },
    ArrayBuffer {
        id: usize,
        bytes: Vec<u8>,
    },
    Buffer {
        id: usize,
        bytes: Vec<u8>,
    },
    /// TypedArray 视图元数据；当前反序列化重建为 Buffer（与 structuredClone 一致）。
    /// `kind`/`element_size`/`byte_offset`/`length` 保留供后续按类型重建。
    TypedArray {
        id: usize,
        #[allow(dead_code)]
        kind: u8,
        #[allow(dead_code)]
        element_size: u8,
        bytes: Vec<u8>,
        #[allow(dead_code)]
        byte_offset: u32,
        #[allow(dead_code)]
        length: u32,
    },
    SharedArrayBuffer {
        id: usize,
        handle: u32,
    },
    /// perf_hooks Histogram：跨 Store 直接传递不可猜测的 backing capability。
    Histogram {
        id: usize,
        capability: crate::runtime_node_perf_hooks_histogram::HistogramCapability,
        /// 0=只读 Histogram，1=Recordable；Interval clone 时序列化为 0。
        kind: u8,
    },
    MessagePort {
        id: usize,
        global_id: u32,
    },
    Ref(usize),
}
