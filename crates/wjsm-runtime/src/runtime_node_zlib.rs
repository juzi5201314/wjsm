use std::io::{Cursor, Read};

use brotli::{CompressorReader, Decompressor};
use flate2::Compression;
use flate2::read::{DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder};
use wasmtime::Caller;

use crate::runtime_buffer::create_buffer_from_bytes;
use crate::runtime_node_data::bytes_from_value;
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ZlibMethodKind {
    GzipSync,
    GunzipSync,
    DeflateSync,
    InflateSync,
    DeflateRawSync,
    InflateRawSync,
    BrotliCompressSync,
    BrotliDecompressSync,
}

impl ZlibMethodKind {
    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::GzipSync),
            1 => Some(Self::GunzipSync),
            2 => Some(Self::DeflateSync),
            3 => Some(Self::InflateSync),
            4 => Some(Self::DeflateRawSync),
            5 => Some(Self::InflateRawSync),
            6 => Some(Self::BrotliCompressSync),
            7 => Some(Self::BrotliDecompressSync),
            _ => None,
        }
    }

    pub(crate) fn method(self) -> u8 {
        match self {
            Self::GzipSync => 0,
            Self::GunzipSync => 1,
            Self::DeflateSync => 2,
            Self::InflateSync => 3,
            Self::DeflateRawSync => 4,
            Self::InflateRawSync => 5,
            Self::BrotliCompressSync => 6,
            Self::BrotliDecompressSync => 7,
        }
    }
}

pub(crate) fn create_zlib_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 8);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_zlib_method(caller, obj, "gzipSync", ZlibMethodKind::GzipSync);
    install_zlib_method(caller, obj, "gunzipSync", ZlibMethodKind::GunzipSync);
    install_zlib_method(caller, obj, "deflateSync", ZlibMethodKind::DeflateSync);
    install_zlib_method(caller, obj, "inflateSync", ZlibMethodKind::InflateSync);
    install_zlib_method(caller, obj, "deflateRawSync", ZlibMethodKind::DeflateRawSync);
    install_zlib_method(caller, obj, "inflateRawSync", ZlibMethodKind::InflateRawSync);
    install_zlib_method(caller, obj, "brotliCompressSync", ZlibMethodKind::BrotliCompressSync);
    install_zlib_method(caller, obj, "brotliDecompressSync", ZlibMethodKind::BrotliDecompressSync);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_zlib_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: ZlibMethodKind,
    args: &[i64],
) -> i64 {
    let Some(input) = args.first().copied() else {
        return make_type_error_exception(caller, "zlib input is required");
    };
    let input = match bytes_from_value(caller, input, "zlib input") {
        Ok(bytes) => bytes,
        Err(error) => return error,
    };
    let result = match kind {
        ZlibMethodKind::GzipSync => gzip(&input),
        ZlibMethodKind::GunzipSync => gunzip(&input),
        ZlibMethodKind::DeflateSync => deflate(&input),
        ZlibMethodKind::InflateSync => inflate(&input),
        ZlibMethodKind::DeflateRawSync => deflate_raw(&input),
        ZlibMethodKind::InflateRawSync => inflate_raw(&input),
        ZlibMethodKind::BrotliCompressSync => brotli_compress(&input),
        ZlibMethodKind::BrotliDecompressSync => brotli_decompress(&input),
    };
    match result {
        Ok(bytes) => create_buffer_from_bytes(caller, bytes),
        Err(error) => make_zlib_error(caller, &error.to_string()),
    }
}

fn install_zlib_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: ZlibMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::ZlibMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn gzip(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = GzEncoder::new(Cursor::new(input), Compression::default());
    read_all(&mut reader)
}

fn gunzip(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = GzDecoder::new(Cursor::new(input));
    read_all(&mut reader)
}

fn deflate(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = ZlibEncoder::new(Cursor::new(input), Compression::default());
    read_all(&mut reader)
}

fn inflate(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = ZlibDecoder::new(Cursor::new(input));
    read_all(&mut reader)
}

fn deflate_raw(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = DeflateEncoder::new(Cursor::new(input), Compression::default());
    read_all(&mut reader)
}

fn inflate_raw(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = DeflateDecoder::new(Cursor::new(input));
    read_all(&mut reader)
}

fn brotli_compress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = CompressorReader::new(input, 4096, 5, 22);
    read_all(&mut reader)
}

fn brotli_decompress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = Decompressor::new(input, 4096);
    read_all(&mut reader)
}

fn read_all(reader: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

fn make_zlib_error(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
    let msg_val = store_runtime_string(caller, msg.to_string());
    let error_obj = create_error_object(caller, "Error", msg_val, value::encode_undefined());
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "Error".to_string(),
        message: msg.to_string(),
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}
