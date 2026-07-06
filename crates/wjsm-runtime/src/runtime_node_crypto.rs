use std::sync::{Arc, Mutex};

use digest::Digest;
use hmac::{Hmac, Mac};
use rand::RngCore;

use crate::runtime_buffer::{arraybuffer_visible_bytes, create_buffer_from_bytes, visible_bytes};
use crate::runtime_encoding::{
    BufferEncoding, decode_bytes, encode_js_string, encoding_from_value, js_string_lossy,
};
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CryptoMethodKind {
    RandomBytes,
    RandomUuid,
    RandomInt,
    CreateHash,
    CreateHmac,
    TimingSafeEqual,
    GetHashes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CryptoDigestKind {
    Update,
    Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CryptoAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

#[derive(Debug)]
pub(crate) struct CryptoDigestState {
    algorithm: CryptoAlgorithm,
    key: Option<Vec<u8>>,
    chunks: Vec<u8>,
    digested: bool,
}

pub(crate) fn create_crypto_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 7);
    install_crypto_method(caller, obj, "randomBytes", CryptoMethodKind::RandomBytes);
    install_crypto_method(caller, obj, "randomUUID", CryptoMethodKind::RandomUuid);
    install_crypto_method(caller, obj, "randomInt", CryptoMethodKind::RandomInt);
    install_crypto_method(caller, obj, "createHash", CryptoMethodKind::CreateHash);
    install_crypto_method(caller, obj, "createHmac", CryptoMethodKind::CreateHmac);
    install_crypto_method(
        caller,
        obj,
        "timingSafeEqual",
        CryptoMethodKind::TimingSafeEqual,
    );
    install_crypto_method(caller, obj, "getHashes", CryptoMethodKind::GetHashes);
    obj
}

pub(crate) fn call_crypto_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: CryptoMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        CryptoMethodKind::RandomBytes => random_bytes(caller, args),
        CryptoMethodKind::RandomUuid => {
            store_runtime_string(caller, uuid::Uuid::new_v4().to_string())
        }
        CryptoMethodKind::RandomInt => random_int(caller, args),
        CryptoMethodKind::CreateHash => create_digest(caller, args, None),
        CryptoMethodKind::CreateHmac => create_hmac(caller, args),
        CryptoMethodKind::TimingSafeEqual => timing_safe_equal(caller, args),
        CryptoMethodKind::GetHashes => get_hashes(caller),
    }
}

pub(crate) fn call_crypto_digest_method(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    state: Arc<Mutex<CryptoDigestState>>,
    kind: CryptoDigestKind,
    args: &[i64],
) -> i64 {
    match kind {
        CryptoDigestKind::Update => digest_update(caller, this_val, state, args),
        CryptoDigestKind::Digest => digest_final(caller, state, args),
    }
}

fn install_crypto_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: CryptoMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::CryptoMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn create_hmac(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let key = match data_bytes(caller, args.get(1).copied(), args.get(2).copied()) {
        Ok(key) => key,
        Err(err) => return err,
    };
    create_digest(caller, args, Some(key))
}

fn create_digest(caller: &mut Caller<'_, RuntimeState>, args: &[i64], key: Option<Vec<u8>>) -> i64 {
    let algorithm_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let algorithm_label = js_string_lossy(caller, algorithm_value);
    let algorithm = match parse_algorithm(&algorithm_label) {
        Some(algorithm) => algorithm,
        None => {
            return make_type_error_exception(
                caller,
                &format!("Digest method not supported: {algorithm_label}"),
            );
        }
    };
    let state = Arc::new(Mutex::new(CryptoDigestState {
        algorithm,
        key,
        chunks: Vec::new(),
        digested: false,
    }));
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let update = create_native_callable(
        caller.data(),
        NativeCallable::CryptoDigestMethod {
            state: Arc::clone(&state),
            kind: CryptoDigestKind::Update,
        },
    );
    let digest = create_native_callable(
        caller.data(),
        NativeCallable::CryptoDigestMethod {
            state,
            kind: CryptoDigestKind::Digest,
        },
    );
    let _ = define_host_data_property_from_caller(caller, obj, "update", update);
    let _ = define_host_data_property_from_caller(caller, obj, "digest", digest);
    obj
}

fn digest_update(
    caller: &mut Caller<'_, RuntimeState>,
    this_val: i64,
    state: Arc<Mutex<CryptoDigestState>>,
    args: &[i64],
) -> i64 {
    let bytes = match data_bytes(caller, args.first().copied(), args.get(1).copied()) {
        Ok(bytes) => bytes,
        Err(err) => return err,
    };
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if state.digested {
        return make_error_exception(caller, "Digest already called");
    }
    state.chunks.extend_from_slice(&bytes);
    this_val
}

fn digest_final(
    caller: &mut Caller<'_, RuntimeState>,
    state: Arc<Mutex<CryptoDigestState>>,
    args: &[i64],
) -> i64 {
    let (algorithm, key, chunks) = {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if state.digested {
            return make_error_exception(caller, "Digest already called");
        }
        state.digested = true;
        (state.algorithm, state.key.clone(), state.chunks.clone())
    };
    let output = compute_digest(algorithm, key.as_deref(), &chunks);
    let encoding = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    if value::is_undefined(encoding) || value::is_null(encoding) {
        return create_buffer_from_bytes(caller, output);
    }
    let label = js_string_lossy(caller, encoding);
    if label.eq_ignore_ascii_case("buffer") {
        return create_buffer_from_bytes(caller, output);
    }
    match encoding_from_value(caller, encoding) {
        Ok(encoding) => decode_bytes(caller, &output, encoding),
        Err(label) => make_type_error_exception(caller, &format!("Unknown encoding: {label}")),
    }
}

fn random_bytes(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let size_value = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let size = value::decode_f64(to_number(caller, size_value));
    if !size.is_finite() || size < 0.0 || size.trunc() > u32::MAX as f64 {
        return make_range_error_exception(caller, "Invalid randomBytes size");
    }
    let mut bytes = vec![0; size.trunc() as usize];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    create_buffer_from_bytes(caller, bytes)
}

fn random_int(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let (min_raw, max_raw) = match args {
        [max] => (value::encode_f64(0.0), *max),
        [min, max, ..] => (*min, *max),
        _ => return make_range_error_exception(caller, "Invalid randomInt range"),
    };
    let min = value::decode_f64(to_number(caller, min_raw));
    let max = value::decode_f64(to_number(caller, max_raw));
    if !is_safe_integer(min) || !is_safe_integer(max) || max <= min {
        return make_range_error_exception(caller, "Invalid randomInt range");
    }
    let range = (max - min) as u64;
    if range == 0 || range > (1_u64 << 48) {
        return make_range_error_exception(caller, "Invalid randomInt range");
    }
    let zone = u64::MAX - (u64::MAX % range);
    loop {
        let candidate = rand::rngs::OsRng.next_u64();
        if candidate < zone {
            let value = min as i64 + (candidate % range) as i64;
            return value::encode_f64(value as f64);
        }
    }
}

fn timing_safe_equal(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let left = match bytes_like(caller, args.first().copied()) {
        Some(bytes) => bytes,
        None => {
            return make_type_error_exception(
                caller,
                "Expected Buffer, Uint8Array, or ArrayBuffer",
            );
        }
    };
    let right = match bytes_like(caller, args.get(1).copied()) {
        Some(bytes) => bytes,
        None => {
            return make_type_error_exception(
                caller,
                "Expected Buffer, Uint8Array, or ArrayBuffer",
            );
        }
    };
    if left.len() != right.len() {
        return make_range_error_exception(caller, "Input buffers must have the same byte length");
    }
    let diff = left
        .iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right));
    value::encode_bool(diff == 0)
}

fn get_hashes(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let hashes = ["md5", "sha1", "sha256", "sha512"];
    let arr = alloc_array(caller, hashes.len() as u32);
    for (index, hash) in hashes.iter().enumerate() {
        let value = store_runtime_string(caller, (*hash).to_string());
        set_array_elem(caller, arr, index as i32, value);
    }
    arr
}

fn data_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    encoding_raw: Option<i64>,
) -> Result<Vec<u8>, i64> {
    let value_raw = value_raw.unwrap_or_else(value::encode_undefined);
    if let Some(bytes) = bytes_like(caller, Some(value_raw)) {
        return Ok(bytes);
    }
    let encoding = match encoding_raw {
        Some(raw) if !value::is_undefined(raw) => {
            encoding_from_value(caller, raw).map_err(|label| {
                make_type_error_exception(caller, &format!("Unknown encoding: {label}"))
            })?
        }
        _ => BufferEncoding::Utf8,
    };
    encode_js_string(caller, value_raw, encoding)
        .map_err(|label| make_type_error_exception(caller, &format!("Unknown encoding: {label}")))
}

fn bytes_like(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> Option<Vec<u8>> {
    let value_raw = value_raw?;
    visible_bytes(caller, value_raw).or_else(|| arraybuffer_visible_bytes(caller, value_raw))
}

fn parse_algorithm(label: &str) -> Option<CryptoAlgorithm> {
    match label.to_ascii_lowercase().as_str() {
        "md5" => Some(CryptoAlgorithm::Md5),
        "sha1" | "sha-1" => Some(CryptoAlgorithm::Sha1),
        "sha256" | "sha-256" => Some(CryptoAlgorithm::Sha256),
        "sha512" | "sha-512" => Some(CryptoAlgorithm::Sha512),
        _ => None,
    }
}

fn compute_digest(algorithm: CryptoAlgorithm, key: Option<&[u8]>, chunks: &[u8]) -> Vec<u8> {
    match key {
        Some(key) => match algorithm {
            CryptoAlgorithm::Md5 => hmac_md5(key, chunks),
            CryptoAlgorithm::Sha1 => hmac_sha1(key, chunks),
            CryptoAlgorithm::Sha256 => hmac_sha256(key, chunks),
            CryptoAlgorithm::Sha512 => hmac_sha512(key, chunks),
        },
        None => match algorithm {
            CryptoAlgorithm::Md5 => md5::Md5::digest(chunks).to_vec(),
            CryptoAlgorithm::Sha1 => sha1::Sha1::digest(chunks).to_vec(),
            CryptoAlgorithm::Sha256 => sha2::Sha256::digest(chunks).to_vec(),
            CryptoAlgorithm::Sha512 => sha2::Sha512::digest(chunks).to_vec(),
        },
    }
}

fn hmac_md5(key: &[u8], chunks: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<md5::Md5>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, chunks);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha1(key: &[u8], chunks: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, chunks);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256(key: &[u8], chunks: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, chunks);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha512(key: &[u8], chunks: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<sha2::Sha512>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, chunks);
    mac.finalize().into_bytes().to_vec()
}

fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
}

fn make_error_exception(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
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
