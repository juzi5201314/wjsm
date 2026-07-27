//! WHATWG Streams 的后端无关状态条目。

use std::collections::VecDeque;

use crate::Value;

#[derive(Clone, Debug)]
pub enum StreamState {
    Readable,
    Closed,
    Errored,
}

#[derive(Clone, Debug)]
pub struct ReadableStreamEntry {
    pub state: StreamState,
    pub error: Option<String>,
    pub disturbed: bool,
    pub locked: bool,
    pub http_response_handle: Option<u32>,
    pub response_body_handle: Option<u32>,
    pub response_body_object: Option<Value>,
    pub controller_handle: Option<u32>,
    pub is_byte_stream: bool,
    pub pipe_to: Option<ReadableStreamPipeToEntry>,
}

#[derive(Clone, Copy, Debug)]
pub struct ReadableStreamPipeToEntry {
    pub destination: u32,
    pub promise: Value,
    pub write_in_flight: bool,
    pub closing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReaderKind {
    Default,
    Byob,
}

#[derive(Clone, Debug)]
pub struct ReaderEntry {
    pub stream_handle: u32,
    pub kind: ReaderKind,
    pub pending_read_promise: Option<Value>,
    pub pending_byob_view: Option<Value>,
    pub closed_promise: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WritableStreamState {
    Writable,
    Closing,
    Closed,
    Errored,
}

#[derive(Debug, Clone)]
pub struct WritableStreamEntry {
    pub state: WritableStreamState,
    pub error: Option<Value>,
    pub locked: bool,
    pub controller_handle: Option<u32>,
    pub abort_signal: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct WriterEntry {
    pub writable_stream_handle: u32,
    pub closed_promise: Option<Value>,
    pub ready_promise: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct TransformStreamEntry {
    pub readable_stream_handle: Option<u32>,
    pub writable_stream_handle: Option<u32>,
    pub transform_callback: Option<Value>,
    pub flush_callback: Option<Value>,
    pub readable_controller_handle: Option<u32>,
    pub transformer_this: Option<Value>,
    pub backpressure: bool,
    pub readable_obj: Option<Value>,
    pub writable_obj: Option<Value>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ControllerKind {
    ReadableDefault,
    Writable,
}

#[derive(Clone)]
pub struct StreamControllerEntry {
    pub kind: ControllerKind,
    pub stream_handle: u32,
    pub chunk_queue: VecDeque<Value>,
    pub high_water_mark: f64,
    pub strategy_size: Option<Value>,
    pub started: bool,
    pub close_requested: bool,
    pub byob_reader_handle: Option<u32>,
    pub pull_requested: bool,
    pub abort_requested: bool,
    pub abort_reason: Option<Value>,
    pub flush_requested: bool,
    pub underlying_source: Option<Value>,
    pub pull_callback: Option<Value>,
    pub write_callback: Option<Value>,
    pub sink_close_callback: Option<Value>,
    pub cancel_callback: Option<Value>,
    pub active_byob_request: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ByobRequestEntry {
    pub controller_handle: u32,
    pub reader_handle: u32,
    pub view: Value,
    pub promise: Value,
    pub responded: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum ReadableStreamMethodKind {
    GetReader,
    GetLocked,
    Cancel,
    Tee,
    AsyncIterator,
    PipeTo,
    PipeThrough,
}

#[derive(Clone, Copy, Debug)]
pub enum ReadableStreamDefaultReaderMethodKind {
    Read,
    ReleaseLock,
    GetClosed,
}

#[derive(Clone, Copy, Debug)]
pub enum ReadableStreamDefaultControllerMethodKind {
    Enqueue,
    Close,
    Error,
    GetDesiredSize,
    GetByobRequest,
}

#[derive(Clone, Copy, Debug)]
pub enum ReadableStreamByobRequestMethodKind {
    GetView,
    Respond,
}

#[derive(Clone, Copy, Debug)]
pub enum TransformStreamMethodKind {
    GetReadable,
    GetWritable,
}

#[derive(Clone, Copy, Debug)]
pub enum WritableStreamMethodKind {
    GetWriter,
    Abort,
    Close,
    GetLocked,
}

#[derive(Clone, Copy, Debug)]
pub enum WritableStreamDefaultWriterMethodKind {
    Write,
    Close,
    Abort,
    ReleaseLock,
    GetClosed,
    GetReady,
    GetDesiredSize,
}

#[derive(Clone, Copy, Debug)]
pub enum WritableStreamDefaultControllerMethodKind {
    Error,
    GetSignal,
}

