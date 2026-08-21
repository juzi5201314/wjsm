use std::collections::{HashMap, HashSet};

use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{
    buffers, collections, date, fail_dispatch, modules, node_buffer, node_perf_hooks, object,
    regexp, runtime, typedarray,
};
use crate::NativeAgentState;

const MAX_CLONE_NODES: usize = 1 << 20;

#[derive(Clone)]
pub(crate) struct SerializedGraph {
    root: CloneValue,
    nodes: Vec<CloneNode>,
}

#[derive(Clone)]
enum CloneValue {
    Undefined,
    Hole,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    BigInt(String),
    Node(u32),
}

#[derive(Clone)]
enum CloneBacking {
    Values(Vec<CloneValue>),
    ArrayBuffer {
        node: u32,
        offset: usize,
        length: usize,
    },
    SharedArrayBuffer {
        node: u32,
        offset: usize,
        length: usize,
    },
}

#[derive(Clone)]
enum CloneNode {
    Pending,
    Array(Vec<CloneValue>),
    Object(Vec<(String, CloneValue)>),
    Histogram(node_perf_hooks::HistogramTransfer),
    ArrayBuffer(Vec<u8>),
    SharedArrayBuffer(u32),
    Date(f64),
    RegExp {
        pattern: String,
        flags: String,
    },
    Map(Vec<(CloneValue, CloneValue)>),
    Set(Vec<CloneValue>),
    TypedArray {
        kind: typedarray::TypedArrayKind,
        backing: CloneBacking,
    },
    DataView {
        backing: CloneBacking,
    },
    Buffer {
        backing: CloneBacking,
    },
}

pub(super) fn dispatch_structured_clone(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let _ = builtin;
    Some(structured_clone(ctx, state, args))
}

pub(crate) fn structured_clone(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let root = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let transfers = match transfer_list(ctx, state, args.get(1).copied()) {
        Ok(transfers) => transfers,
        Err(message) => return data_clone_error(ctx, state, &message),
    };
    let graph = match serialize(ctx, state, root) {
        Ok(graph) => graph,
        Err(message) => return data_clone_error(ctx, state, &message),
    };
    let result = match deserialize(state, &graph) {
        Ok(result) => result,
        Err(message) => return data_clone_error(ctx, state, &message),
    };
    for handle in transfers {
        buffers::detach(state, handle);
    }
    result
}

pub(crate) fn clone_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
) -> i64 {
    match serialize(ctx, state, encoded).and_then(|graph| deserialize(state, &graph)) {
        Ok(value) => value,
        Err(message) => data_clone_error(ctx, state, &message),
    }
}

pub(crate) fn data_clone_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    message: &str,
) -> i64 {
    modules::named_error_object(state, "DataCloneError", message.to_owned())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(crate) fn serialize(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    root: i64,
) -> Result<SerializedGraph, String> {
    serialize_graph(ctx, state, root)
}

pub(crate) fn deserialize(
    state: &mut NativeAgentState,
    graph: &SerializedGraph,
) -> Result<i64, String> {
    let mut objects = vec![None; graph.nodes.len()];
    for (index, node) in graph.nodes.iter().enumerate() {
        objects[index] = Some(match node {
            CloneNode::Pending => return Err("DataCloneError: incomplete object graph".into()),
            CloneNode::Array(elements) => state
                .allocate_object(
                    u32::try_from(elements.len())
                        .map_err(|_| "DataCloneError: object graph is too large".to_string())?,
                    true,
                )
                .map_err(|error| error.to_string())?,
            CloneNode::Object(properties) => state
                .allocate_object(
                    u32::try_from(properties.len())
                        .map_err(|_| "DataCloneError: object graph is too large".to_string())?,
                    false,
                )
                .map_err(|error| error.to_string())?,
            CloneNode::Histogram(histogram) => {
                node_perf_hooks::materialize_histogram(state, histogram.clone())?
            }
            CloneNode::ArrayBuffer(bytes) => buffers::from_bytes(state, bytes.clone())
                .ok_or_else(|| "DataCloneError: ArrayBuffer allocation failed".to_string())?,
            CloneNode::SharedArrayBuffer(backing_id) => {
                let object = state
                    .allocate_object(0, false)
                    .map_err(|error| error.to_string())?;
                state.insert_shared_array_buffer(value::decode_handle(object), *backing_id);
                if !state
                    .shared_array_buffers
                    .contains_key(&value::decode_handle(object))
                {
                    return Err("DataCloneError: SharedArrayBuffer backing is unavailable".into());
                }
                object
            }
            CloneNode::Date(milliseconds) => date::from_millis(state, *milliseconds)
                .ok_or_else(|| "DataCloneError: Date allocation failed".to_string())?,
            CloneNode::RegExp { pattern, flags } => {
                regexp::from_parts(state, pattern.clone(), flags.clone())
                    .ok_or_else(|| "DataCloneError: RegExp allocation failed".to_string())?
            }
            CloneNode::Map(_) => collections::create_map(state)
                .ok_or_else(|| "DataCloneError: Map allocation failed".to_string())?,
            CloneNode::Set(_) => collections::create_set(state)
                .ok_or_else(|| "DataCloneError: Set allocation failed".to_string())?,
            CloneNode::TypedArray { kind, backing } => match backing {
                CloneBacking::Values(values) => {
                    let values = values
                        .iter()
                        .map(|value| deserialize_value(state, value, &objects))
                        .collect::<Result<Vec<_>, _>>()?;
                    typedarray::from_values(state, *kind, values).ok_or_else(|| {
                        "DataCloneError: typed array allocation failed".to_string()
                    })?
                }
                CloneBacking::ArrayBuffer { .. } | CloneBacking::SharedArrayBuffer { .. } => {
                    continue;
                }
            },
            CloneNode::DataView { .. } | CloneNode::Buffer { .. } => continue,
        });
    }

    for (index, node) in graph.nodes.iter().enumerate() {
        if objects[index].is_some() {
            continue;
        }
        objects[index] = Some(match node {
            CloneNode::TypedArray { kind, backing } => {
                let (buffer, offset, length) = resolve_backing(backing, &objects)?;
                match backing {
                    CloneBacking::ArrayBuffer { .. } => {
                        typedarray::from_buffer(state, *kind, buffer, offset, length)
                    }
                    CloneBacking::SharedArrayBuffer { .. } => {
                        typedarray::from_shared_buffer(state, *kind, buffer, offset, length)
                    }
                    CloneBacking::Values(_) => None,
                }
                .ok_or_else(|| "DataCloneError: typed array allocation failed".to_string())?
            }
            CloneNode::DataView { backing } => {
                let (buffer, offset, length) = resolve_backing(backing, &objects)?;
                buffers::from_view(state, buffer, offset, length)
                    .ok_or_else(|| "DataCloneError: DataView allocation failed".to_string())?
            }
            CloneNode::Buffer { backing } => {
                let (buffer, offset, length) = resolve_backing(backing, &objects)?;
                node_buffer::from_array_buffer_view(state, buffer, offset, length)
                    .ok_or_else(|| "DataCloneError: Buffer allocation failed".to_string())?
            }
            _ => return Err("DataCloneError: invalid deferred node".into()),
        });
    }

    for (index, node) in graph.nodes.iter().enumerate() {
        let object = objects[index].ok_or_else(|| {
            "DataCloneError: object graph contains an unresolved reference".to_string()
        })?;
        match node {
            CloneNode::Array(elements) => {
                for element in elements {
                    let stored = deserialize_value(state, element, &objects)?;
                    state
                        .gc
                        .heap()
                        .push_element(value::decode_handle(object), stored as u64)
                        .map_err(|error| error.to_string())?;
                }
            }
            CloneNode::Object(properties) => {
                for (name, stored) in properties {
                    let key = state
                        .intern_property_string(name.clone().into())
                        .ok_or_else(|| "DataCloneError: string table overflow".to_string())?;
                    let stored = deserialize_value(state, stored, &objects)?;
                    state
                        .gc
                        .heap()
                        .set_property(value::decode_handle(object), key, stored as u64)
                        .map_err(|error| error.to_string())?;
                }
            }
            CloneNode::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            deserialize_value(state, key, &objects)?,
                            deserialize_value(state, value, &objects)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                collections::insert_map_entries(state, object, entries)
                    .then_some(())
                    .ok_or_else(|| "DataCloneError: Map population failed".to_string())?;
            }
            CloneNode::Set(values) => {
                let values = values
                    .iter()
                    .map(|value| deserialize_value(state, value, &objects))
                    .collect::<Result<Vec<_>, String>>()?;
                collections::insert_set_values(state, object, values)
                    .then_some(())
                    .ok_or_else(|| "DataCloneError: Set population failed".to_string())?;
            }
            CloneNode::Pending
            | CloneNode::Histogram(_)
            | CloneNode::ArrayBuffer(_)
            | CloneNode::SharedArrayBuffer(_)
            | CloneNode::Date(_)
            | CloneNode::RegExp { .. }
            | CloneNode::TypedArray { .. }
            | CloneNode::DataView { .. }
            | CloneNode::Buffer { .. } => {}
        }
    }
    deserialize_value(state, &graph.root, &objects)
}

fn transfer_list(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    options: Option<i64>,
) -> Result<Vec<u32>, String> {
    let Some(options) = options else {
        return Ok(Vec::new());
    };
    if value::is_undefined(options) {
        return Ok(Vec::new());
    }
    if value::is_null(options) || !value::is_js_object(options) {
        return Err("DataCloneError: options must be an object".into());
    }
    let key = state
        .intern_text("transfer".into(), value::TAG_STRING)
        .ok_or_else(|| "DataCloneError: string table overflow".to_string())?;
    let transfer = runtime::get_property(ctx, state, options, key)
        .map_err(|()| "DataCloneError: transfer list getter failed".to_string())?;
    if value::is_undefined(transfer) {
        return Ok(Vec::new());
    }
    if !value::is_array(transfer) {
        return Err("DataCloneError: transfer must be an array".into());
    }
    let length = state
        .gc
        .heap()
        .array_length(value::decode_handle(transfer))
        .map_err(|error| error.to_string())?;
    let mut seen = HashSet::new();
    let mut handles = Vec::with_capacity(length as usize);
    for index in 0..length {
        let encoded = state
            .gc
            .heap()
            .get_element(value::decode_handle(transfer), index)
            .map_err(|error| error.to_string())?
            .map(|value| value as i64)
            .filter(|value| !value::is_array_hole(*value))
            .ok_or_else(|| "DataCloneError: transfer list contains a hole".to_string())?;
        let handle = if value::is_js_object(encoded) {
            value::decode_handle(encoded)
        } else {
            return Err("DataCloneError: transfer list item is not transferable".into());
        };
        if !state.array_buffers.contains_key(&handle) {
            return Err("DataCloneError: transfer list item is not transferable".into());
        }
        if !seen.insert(handle) {
            return Err("DataCloneError: transfer list contains duplicates".into());
        }
        handles.push(handle);
    }
    Ok(handles)
}

fn serialize_graph(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    root: i64,
) -> Result<SerializedGraph, String> {
    let mut nodes = Vec::new();
    let mut seen = HashMap::new();
    let mut pending = Vec::new();
    let root = serialize_value(state, root, &mut nodes, &mut seen, &mut pending)?;
    while let Some((node, encoded)) = pending.pop() {
        let serialized = serialize_node(ctx, state, encoded, &mut nodes, &mut seen, &mut pending)?;
        *nodes
            .get_mut(
                usize::try_from(node)
                    .map_err(|_| "DataCloneError: node id overflow".to_string())?,
            )
            .ok_or_else(|| "DataCloneError: invalid object node".to_string())? = serialized;
    }
    Ok(SerializedGraph { root, nodes })
}

fn serialize_value(
    state: &mut NativeAgentState,
    encoded: i64,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneValue, String> {
    if value::is_undefined(encoded) {
        return Ok(CloneValue::Undefined);
    }
    if value::is_array_hole(encoded) {
        return Ok(CloneValue::Hole);
    }
    if value::is_null(encoded) {
        return Ok(CloneValue::Null);
    }
    if value::is_bool(encoded) {
        return Ok(CloneValue::Bool(value::decode_bool(encoded)));
    }
    if value::is_f64(encoded) {
        return Ok(CloneValue::Number(value::decode_f64(encoded)));
    }
    if value::is_string(encoded) {
        return state.string_owned(encoded)
            .and_then(|text| text.to_utf8())
            .map(CloneValue::String)
            .ok_or_else(|| "DataCloneError: invalid string".to_string());
    }
    if value::is_bigint(encoded) {
        return state.string_owned(encoded)
            .and_then(|text| text.to_utf8())
            .map(CloneValue::BigInt)
            .ok_or_else(|| "DataCloneError: invalid bigint".to_string());
    }
    if value::is_symbol(encoded) {
        return Err("DataCloneError: symbol cannot be cloned".into());
    }
    if value::is_proxy(encoded) || value::is_callable(encoded) || value::is_exception(encoded) {
        return Err("DataCloneError: value cannot be cloned".into());
    }
    if value::is_regexp(encoded) || value::is_array(encoded) || value::is_js_object(encoded) {
        return reserve_node(encoded, nodes, seen, pending);
    }
    Err("DataCloneError: value cannot be cloned".into())
}

fn reserve_node(
    encoded: i64,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneValue, String> {
    if let Some(node) = seen.get(&encoded).copied() {
        return Ok(CloneValue::Node(node));
    }
    let node = u32::try_from(nodes.len())
        .map_err(|_| "DataCloneError: object graph is too large".to_string())?;
    if nodes.len() >= MAX_CLONE_NODES {
        return Err("DataCloneError: object graph is too large".into());
    }
    nodes.push(CloneNode::Pending);
    seen.insert(encoded, node);
    pending.push((node, encoded));
    Ok(CloneValue::Node(node))
}

fn serialize_node(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    encoded: i64,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneNode, String> {
    if value::is_regexp(encoded) {
        let (pattern, flags) = regexp::clone_parts(state, encoded)
            .ok_or_else(|| "DataCloneError: invalid RegExp".to_string())?;
        return Ok(CloneNode::RegExp { pattern, flags });
    }
    if value::is_array(encoded) {
        let length = state
            .gc
            .heap()
            .array_length(value::decode_handle(encoded))
            .map_err(|error| error.to_string())?;
        let mut elements = Vec::with_capacity(length as usize);
        for index in 0..length {
            let stored = state
                .gc
                .heap()
                .get_element(value::decode_handle(encoded), index)
                .map_err(|error| error.to_string())?;
            elements.push(match stored.map(|value| value as i64) {
                Some(value) if !value::is_array_hole(value) => {
                    serialize_value(state, value, nodes, seen, pending)?
                }
                _ => CloneValue::Hole,
            });
        }
        return Ok(CloneNode::Array(elements));
    }
    let handle = value::decode_handle(encoded);
    if state.promises.contains_key(&handle) {
        return Err("DataCloneError: Promise cannot be cloned".into());
    }
    if let Some(shared) = state.shared_array_buffers.get(&handle) {
        return Ok(CloneNode::SharedArrayBuffer(shared.backing_id));
    }
    if let Some(histogram) = node_perf_hooks::transfer_histogram(state, encoded) {
        return Ok(CloneNode::Histogram(histogram));
    }
    if node_buffer::bytes(state, encoded).is_some() {
        return Ok(CloneNode::Buffer {
            backing: serialize_buffer_backing(state, encoded, nodes, seen, pending)?,
        });
    }
    if let Some(bytes) = buffers::array_buffer_bytes(state, encoded) {
        return Ok(CloneNode::ArrayBuffer(bytes));
    }
    if let Some((milliseconds, _)) = date::parts(state, encoded) {
        return Ok(CloneNode::Date(milliseconds));
    }
    if let Some((kind, view)) = typedarray::clone_view(state, encoded) {
        return Ok(CloneNode::TypedArray {
            kind,
            backing: serialize_typed_backing(state, view, nodes, seen, pending)?,
        });
    }
    if let Some((backing, offset, length)) = buffers::data_view_parts(state, encoded) {
        return Ok(CloneNode::DataView {
            backing: serialize_view_backing(state, backing, offset, length, nodes, seen, pending)?,
        });
    }
    if let Some(entries) = collections::map_entries(state, encoded) {
        let entries = entries
            .into_iter()
            .map(|(key, value)| {
                Ok((
                    serialize_value(state, key, nodes, seen, pending)?,
                    serialize_value(state, value, nodes, seen, pending)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        return Ok(CloneNode::Map(entries));
    }
    if let Some(values) = collections::set_values(state, encoded) {
        let values = values
            .into_iter()
            .map(|value| serialize_value(state, value, nodes, seen, pending))
            .collect::<Result<Vec<_>, String>>()?;
        return Ok(CloneNode::Set(values));
    }
    let properties = object::own_keys(state, encoded, true)
        .ok_or_else(|| "DataCloneError: object properties are unavailable".to_string())?;
    let mut cloned = Vec::with_capacity(properties.len());
    for (key, _) in properties {
        if !value::is_string(key) {
            return Err("DataCloneError: symbol keys cannot be cloned".into());
        }
        let name = state.string_owned(key)
            .and_then(|name| name.to_utf8())
            .ok_or_else(|| "DataCloneError: invalid property key".to_string())?;
        let stored = runtime::get_property(ctx, state, encoded, key)
            .map_err(|()| "DataCloneError: property getter failed".to_string())?;
        cloned.push((name, serialize_value(state, stored, nodes, seen, pending)?));
    }
    Ok(CloneNode::Object(cloned))
}

fn serialize_buffer_backing(
    state: &mut NativeAgentState,
    encoded: i64,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneBacking, String> {
    let (buffer, offset, length) = node_buffer::parts(state, encoded)
        .ok_or_else(|| "DataCloneError: invalid Buffer".to_string())?;
    let backing = serialize_buffer_reference(state, buffer, nodes, seen, pending)?;
    Ok(backing.map_offset(offset, length))
}

fn serialize_typed_backing(
    state: &mut NativeAgentState,
    view: typedarray::CloneView,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneBacking, String> {
    match view {
        typedarray::CloneView::Values(values) => values
            .into_iter()
            .map(|value| serialize_value(state, value, nodes, seen, pending))
            .collect::<Result<Vec<_>, String>>()
            .map(CloneBacking::Values),
        typedarray::CloneView::ArrayBuffer {
            buffer,
            offset,
            length,
        } => serialize_buffer_reference(state, buffer, nodes, seen, pending)
            .map(|backing| backing.map_offset(offset, length)),
        typedarray::CloneView::SharedArrayBuffer {
            object,
            offset,
            length,
        } => serialize_buffer_reference(state, object, nodes, seen, pending)
            .map(|backing| backing.map_shared_offset(offset, length)),
    }
}

fn serialize_view_backing(
    state: &mut NativeAgentState,
    backing: buffers::ViewBacking,
    offset: usize,
    length: usize,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneBacking, String> {
    let encoded = match backing {
        buffers::ViewBacking::ArrayBuffer(object)
        | buffers::ViewBacking::SharedArrayBuffer(object) => object,
    };
    let backing = serialize_buffer_reference(state, encoded, nodes, seen, pending)?;
    Ok(match backing {
        CloneBacking::ArrayBuffer { node, .. } => CloneBacking::ArrayBuffer {
            node,
            offset,
            length,
        },
        CloneBacking::SharedArrayBuffer { node, .. } => CloneBacking::SharedArrayBuffer {
            node,
            offset,
            length,
        },
        CloneBacking::Values(_) => {
            return Err("DataCloneError: invalid view backing".into());
        }
    })
}

fn serialize_buffer_reference(
    state: &mut NativeAgentState,
    buffer: i64,
    nodes: &mut Vec<CloneNode>,
    seen: &mut HashMap<i64, u32>,
    pending: &mut Vec<(u32, i64)>,
) -> Result<CloneBacking, String> {
    let node = serialize_value(state, buffer, nodes, seen, pending)?;
    let node = match node {
        CloneValue::Node(node) => node,
        _ => return Err("DataCloneError: invalid view backing".into()),
    };
    if state
        .shared_array_buffers
        .contains_key(&value::decode_handle(buffer))
    {
        Ok(CloneBacking::SharedArrayBuffer {
            node,
            offset: 0,
            length: 0,
        })
    } else {
        Ok(CloneBacking::ArrayBuffer {
            node,
            offset: 0,
            length: 0,
        })
    }
}

fn resolve_backing(
    backing: &CloneBacking,
    objects: &[Option<i64>],
) -> Result<(i64, usize, usize), String> {
    match backing {
        CloneBacking::ArrayBuffer {
            node,
            offset,
            length,
        }
        | CloneBacking::SharedArrayBuffer {
            node,
            offset,
            length,
        } => objects
            .get(*node as usize)
            .and_then(|object| *object)
            .map(|object| (object, *offset, *length))
            .ok_or_else(|| "DataCloneError: invalid view backing reference".into()),
        CloneBacking::Values(_) => Err("DataCloneError: view has no backing".into()),
    }
}

fn deserialize_value(
    state: &mut NativeAgentState,
    value: &CloneValue,
    objects: &[Option<i64>],
) -> Result<i64, String> {
    Ok(match value {
        CloneValue::Undefined => value::encode_undefined(),
        CloneValue::Hole => value::encode_array_hole(),
        CloneValue::Null => value::encode_null(),
        CloneValue::Bool(boolean) => value::encode_bool(*boolean),
        CloneValue::Number(number) => value::encode_f64(*number),
        CloneValue::String(text) => state
            .intern_text(text.clone(), value::TAG_STRING)
            .ok_or_else(|| "DataCloneError: string table overflow".to_string())?,
        CloneValue::BigInt(bigint) => state
            .intern_text(bigint.clone(), value::TAG_BIGINT)
            .ok_or_else(|| "DataCloneError: string table overflow".to_string())?,
        CloneValue::Node(node) => objects
            .get(*node as usize)
            .and_then(|object| *object)
            .ok_or_else(|| "DataCloneError: invalid object reference".to_string())?,
    })
}

impl CloneBacking {
    fn map_offset(self, offset: usize, length: usize) -> Self {
        match self {
            Self::ArrayBuffer { node, .. } => Self::ArrayBuffer {
                node,
                offset,
                length,
            },
            Self::SharedArrayBuffer { node, .. } => Self::SharedArrayBuffer {
                node,
                offset,
                length,
            },
            Self::Values(values) => Self::Values(values),
        }
    }

    fn map_shared_offset(self, offset: usize, length: usize) -> Self {
        match self {
            Self::ArrayBuffer { node, .. } | Self::SharedArrayBuffer { node, .. } => {
                Self::SharedArrayBuffer {
                    node,
                    offset,
                    length,
                }
            }
            Self::Values(values) => Self::Values(values),
        }
    }
}
