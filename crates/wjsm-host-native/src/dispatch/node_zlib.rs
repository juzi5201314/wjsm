use std::io::{Cursor, Read, Write};

use brotli::{CompressorReader, Decompressor};
use flate2::Compression;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use wjsm_native_abi::NativeVmContext;

use super::modules;
use super::runtime::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeZlibMethod {
    GzipSync,
    GunzipSync,
    DeflateSync,
    InflateSync,
    DeflateRawSync,
    InflateRawSync,
    BrotliCompressSync,
    BrotliDecompressSync,
}

#[derive(Default)]
pub(crate) struct NodeZlibState {
    pub(crate) bridge: Option<i64>,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_zlib.bridge {
        return Some(bridge);
    }
    let methods = [
        ("gzipSync", NodeZlibMethod::GzipSync),
        ("gunzipSync", NodeZlibMethod::GunzipSync),
        ("deflateSync", NodeZlibMethod::DeflateSync),
        ("inflateSync", NodeZlibMethod::InflateSync),
        ("deflateRawSync", NodeZlibMethod::DeflateRawSync),
        ("inflateRawSync", NodeZlibMethod::InflateRawSync),
        ("brotliCompressSync", NodeZlibMethod::BrotliCompressSync),
        ("brotliDecompressSync", NodeZlibMethod::BrotliDecompressSync),
    ];
    let Ok(capacity) = u32::try_from(methods.len()) else {
        return None;
    };
    let bridge = state.allocate_object(capacity, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeZlib(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_zlib.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeZlibMethod,
    args: &[i64],
) -> i64 {
    let Some(input) = args
        .first()
        .and_then(|encoded| super::node_buffer::bytes(state, *encoded))
    else {
        return type_error(ctx, state, "Buffer argument is required");
    };
    let result = match method {
        NodeZlibMethod::GzipSync => gzip(&input),
        NodeZlibMethod::GunzipSync => gunzip(&input),
        NodeZlibMethod::DeflateSync => deflate(&input),
        NodeZlibMethod::InflateSync => inflate(&input),
        NodeZlibMethod::DeflateRawSync => deflate_raw(&input),
        NodeZlibMethod::InflateRawSync => inflate_raw(&input),
        NodeZlibMethod::BrotliCompressSync => brotli_compress(&input),
        NodeZlibMethod::BrotliDecompressSync => brotli_decompress(&input),
    };
    match result {
        Ok(bytes) => {
            super::node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
        }
        Err(message) => error(ctx, state, message),
    }
}

fn gzip(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

fn gunzip(input: &[u8]) -> Result<Vec<u8>, String> {
    decode_flate(GzDecoder::new(Cursor::new(input)))
}

fn deflate(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

fn inflate(input: &[u8]) -> Result<Vec<u8>, String> {
    decode_flate(ZlibDecoder::new(Cursor::new(input)))
}

fn deflate_raw(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

fn inflate_raw(input: &[u8]) -> Result<Vec<u8>, String> {
    decode_flate(DeflateDecoder::new(Cursor::new(input)))
}

fn decode_flate<R: Read>(mut decoder: R) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn brotli_compress(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = CompressorReader::new(Cursor::new(input), 4096, 5, 22);
    decode_flate(&mut reader)
}

fn brotli_decompress(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = Decompressor::new(Cursor::new(input), 4096);
    decode_flate(&mut reader)
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    error(ctx, state, message.to_owned())
}

fn error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: String) -> i64 {
    modules::named_error_object(state, "Error", message)
        .and_then(|object| state.create_exception(object))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
