use std::sync::{Arc, Mutex};

use crate::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Error,
    Opaque,
    OpaqueRedirect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeadersGuard {
    #[default]
    None,
    Request,
    RequestNoCors,
    Response,
    Immutable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
    Navigate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestCredentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestCache {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

#[derive(Clone, Debug)]
pub struct HeadersEntry {
    pub pairs: Vec<(String, String)>,
    pub guard: HeadersGuard,
}

#[derive(Clone, Debug)]
pub struct FetchResponseEntry {
    pub status: u16,
    pub status_text: String,
    pub headers_handle: u32,
    pub headers_object: Option<Value>,
    pub url: String,
    pub body: Vec<u8>,
    pub response_type: ResponseType,
    pub redirected: bool,
    pub body_used: bool,
    pub http_response_handle: Option<u32>,
    pub stream_handle: Option<u32>,
    pub resource_timing: Option<SharedFetchResourceTiming>,
}

pub type SharedFetchResourceTiming = Arc<Mutex<FetchResourceTimingState>>;
#[derive(Clone, Debug)]
pub struct FetchResourceTimingState {
    pub requested_url: String,
    pub start_time: f64,
    pub request_start_time: f64,
    pub response_start_time: f64,
    pub response_status: u16,
    pub encoded_body_size: u64,
    pub decoded_body_size: u64,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct FetchRequestEntry {
    pub method: String,
    pub url: String,
    pub headers_handle: u32,
    pub headers_object: Option<Value>,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    pub body_used: bool,
    pub signal_handle: Option<u32>,
    pub mode: RequestMode,
    pub credentials: RequestCredentials,
    pub cache: RequestCache,
    pub referrer: String,
    pub referrer_policy: String,
    pub integrity: String,
    pub keepalive: bool,
    pub destination: String,
    pub duplex: String,
}

#[derive(Clone, Debug)]
pub struct AbortSignalEntry {
    pub aborted: bool,
    pub reason: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct HttpRequestSpec {
    pub method: String,
    pub url: String,
    pub headers_handle: u32,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    pub signal_handle: Option<u32>,
    pub resource_timing: Option<SharedFetchResourceTiming>,
}

#[derive(Clone, Copy, Debug)]
pub enum HeadersMethodKind {
    Get,
    Set,
    Has,
    Delete,
    Append,
    Entries,
    ForEach,
    Keys,
    Values,
}

#[derive(Clone, Copy, Debug)]
pub enum ResponseMethodKind {
    Text,
    Json,
    ArrayBuffer,
    Clone,
}

#[derive(Clone, Copy, Debug)]
pub enum RequestMethodKind {
    Clone,
}
