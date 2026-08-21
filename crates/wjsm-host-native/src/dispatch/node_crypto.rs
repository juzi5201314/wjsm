use base64::Engine;
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::{fail_dispatch, modules, node_buffer};
use crate::{NativeAgentState, NativeCallableKind};

type HmacMd5 = Hmac<md5::Md5>;
type HmacSha1 = Hmac<Sha1>;
type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeCryptoCallable {
    CreateHash,
    CreateHmac,
    Digest(u32),
    RandomBytes,
    RandomInt,
    RandomUuid,
    TimingSafeEqual,
    Update(u32),
}

#[derive(Default)]
pub(crate) struct NodeCryptoState {
    bridge: Option<i64>,
    contexts: Vec<CryptoContext>,
}

struct CryptoContext {
    object: i64,
    state: Option<DigestState>,
}

enum DigestState {
    Md5(md5::Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
    HmacMd5(HmacMd5),
    HmacSha1(HmacSha1),
    HmacSha256(HmacSha256),
    HmacSha512(HmacSha512),
}

impl DigestState {
    fn hash(algorithm: &str) -> Option<Self> {
        match normalize_algorithm(algorithm).as_str() {
            "md5" => Some(Self::Md5(md5::Md5::new())),
            "sha1" => Some(Self::Sha1(Sha1::new())),
            "sha256" => Some(Self::Sha256(Sha256::new())),
            "sha512" => Some(Self::Sha512(Sha512::new())),
            _ => None,
        }
    }

    fn hmac(algorithm: &str, key: &[u8]) -> Option<Self> {
        match normalize_algorithm(algorithm).as_str() {
            "md5" => <HmacMd5 as Mac>::new_from_slice(key)
                .ok()
                .map(Self::HmacMd5),
            "sha1" => <HmacSha1 as Mac>::new_from_slice(key)
                .ok()
                .map(Self::HmacSha1),
            "sha256" => <HmacSha256 as Mac>::new_from_slice(key)
                .ok()
                .map(Self::HmacSha256),
            "sha512" => <HmacSha512 as Mac>::new_from_slice(key)
                .ok()
                .map(Self::HmacSha512),
            _ => None,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(state) => Digest::update(state, bytes),
            Self::Sha1(state) => Digest::update(state, bytes),
            Self::Sha256(state) => Digest::update(state, bytes),
            Self::Sha512(state) => Digest::update(state, bytes),
            Self::HmacMd5(state) => Mac::update(state, bytes),
            Self::HmacSha1(state) => Mac::update(state, bytes),
            Self::HmacSha256(state) => Mac::update(state, bytes),
            Self::HmacSha512(state) => Mac::update(state, bytes),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            Self::Md5(state) => state.finalize().to_vec(),
            Self::Sha1(state) => state.finalize().to_vec(),
            Self::Sha256(state) => state.finalize().to_vec(),
            Self::Sha512(state) => state.finalize().to_vec(),
            Self::HmacMd5(state) => state.finalize().into_bytes().to_vec(),
            Self::HmacSha1(state) => state.finalize().into_bytes().to_vec(),
            Self::HmacSha256(state) => state.finalize().into_bytes().to_vec(),
            Self::HmacSha512(state) => state.finalize().into_bytes().to_vec(),
        }
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_crypto.bridge {
        return Some(bridge);
    }
    let methods = [
        ("createHash", NodeCryptoCallable::CreateHash),
        ("createHmac", NodeCryptoCallable::CreateHmac),
        ("randomBytes", NodeCryptoCallable::RandomBytes),
        ("randomInt", NodeCryptoCallable::RandomInt),
        ("randomUUID", NodeCryptoCallable::RandomUuid),
        ("timingSafeEqual", NodeCryptoCallable::TimingSafeEqual),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodeCrypto(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    state.node_crypto.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: NodeCryptoCallable,
    args: &[i64],
) -> i64 {
    match callable {
        NodeCryptoCallable::CreateHash => create_context(ctx, state, args, false),
        NodeCryptoCallable::CreateHmac => create_context(ctx, state, args, true),
        NodeCryptoCallable::Digest(context) => digest(ctx, state, context, args),
        NodeCryptoCallable::RandomBytes => random_bytes(ctx, state, args),
        NodeCryptoCallable::RandomInt => random_int(ctx, state, args),
        NodeCryptoCallable::RandomUuid => random_uuid(ctx, state),
        NodeCryptoCallable::TimingSafeEqual => timing_safe_equal(ctx, state, args),
        NodeCryptoCallable::Update(context) => update(ctx, state, context, args),
    }
}

fn create_context(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    hmac: bool,
) -> i64 {
    let Some(algorithm) = args.first().and_then(|value| string(state, *value)) else {
        return type_error(ctx, state, "algorithm must be a string");
    };
    let digest = if hmac {
        let Some(key) = args.get(1).and_then(|value| input_bytes(state, *value)) else {
            return type_error(ctx, state, "key must be a string or Buffer");
        };
        DigestState::hmac(&algorithm, &key)
    } else {
        DigestState::hash(&algorithm)
    };
    let Some(digest) = digest else {
        return type_error(ctx, state, "Digest method not supported");
    };
    let Ok(context) = u32::try_from(state.node_crypto.contexts.len()) else {
        return fail_dispatch(ctx);
    };
    let Ok(object) = state.allocate_object(2, false) else {
        return fail_dispatch(ctx);
    };
    let Some(update) = state.native_callable(NativeCallableKind::NodeCrypto(
        NodeCryptoCallable::Update(context),
    )) else {
        return fail_dispatch(ctx);
    };
    let Some(digest_method) = state.native_callable(NativeCallableKind::NodeCrypto(
        NodeCryptoCallable::Digest(context),
    )) else {
        return fail_dispatch(ctx);
    };
    if modules::set_named_property(state, object, "update", update).is_err()
        || modules::set_named_property(state, object, "digest", digest_method).is_err()
    {
        return fail_dispatch(ctx);
    }
    state.node_crypto.contexts.push(CryptoContext {
        object,
        state: Some(digest),
    });
    object
}

fn update(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    context: u32,
    args: &[i64],
) -> i64 {
    let Some(bytes) = args.first().and_then(|value| input_bytes(state, *value)) else {
        return type_error(ctx, state, "data must be a string or Buffer");
    };
    let Some(context) = state.node_crypto.contexts.get_mut(context as usize) else {
        return fail_dispatch(ctx);
    };
    let Some(digest) = context.state.as_mut() else {
        return type_error(ctx, state, "Digest already called");
    };
    digest.update(&bytes);
    context.object
}

fn digest(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    context: u32,
    args: &[i64],
) -> i64 {
    let Some(context) = state.node_crypto.contexts.get_mut(context as usize) else {
        return fail_dispatch(ctx);
    };
    let Some(digest) = context.state.take() else {
        return type_error(ctx, state, "Digest already called");
    };
    let bytes = digest.finalize();
    let encoding = args.first().and_then(|value| string(state, *value));
    match encoding.as_deref() {
        None => node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx)),
        Some("hex") => intern(state, hex(&bytes)).unwrap_or_else(|| fail_dispatch(ctx)),
        Some("base64") => intern(
            state,
            base64::engine::general_purpose::STANDARD.encode(bytes),
        )
        .unwrap_or_else(|| fail_dispatch(ctx)),
        Some("base64url") => intern(
            state,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        )
        .unwrap_or_else(|| fail_dispatch(ctx)),
        _ => type_error(ctx, state, "Unknown encoding"),
    }
}

fn random_bytes(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(size) = args.first().and_then(|value| unsigned_integer(*value)) else {
        return type_error(ctx, state, "size must be a non-negative integer");
    };
    let Ok(size) = usize::try_from(size) else {
        return range_error(ctx, state, "size is too large");
    };
    let mut bytes = vec![0; size];
    OsRng.fill_bytes(&mut bytes);
    node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
}

fn random_uuid(ctx: &mut NativeVmContext, state: &mut NativeAgentState) -> i64 {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let text = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    );
    intern(state, text).unwrap_or_else(|| fail_dispatch(ctx))
}

fn random_int(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let (minimum, maximum) = match args {
        [maximum] => (Some(0), signed_integer(*maximum)),
        [minimum, maximum, ..] => (signed_integer(*minimum), signed_integer(*maximum)),
        _ => return type_error(ctx, state, "randomInt requires a range"),
    };
    let (Some(minimum), Some(maximum)) = (minimum, maximum) else {
        return type_error(ctx, state, "range bounds must be integers");
    };
    let Some(width) = maximum.checked_sub(minimum).filter(|width| *width > 0) else {
        return range_error(ctx, state, "maximum must be greater than minimum");
    };
    if width > (1_i64 << 48) {
        return range_error(ctx, state, "range exceeds 2^48");
    }
    let width = width as u64;
    let threshold = width.wrapping_neg() % width;
    let offset = loop {
        let sample = OsRng.next_u64();
        if sample >= threshold {
            break (sample % width) as i64;
        }
    };
    value::encode_f64((minimum + offset) as f64)
}

fn timing_safe_equal(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let [left, right] = args else {
        return type_error(ctx, state, "timingSafeEqual requires two buffers");
    };
    let (Some(left), Some(right)) = (
        node_buffer::bytes(state, *left),
        node_buffer::bytes(state, *right),
    ) else {
        return type_error(ctx, state, "arguments must be Buffers");
    };
    if left.len() != right.len() {
        return range_error(ctx, state, "buffer lengths must match");
    }
    let difference = left
        .iter()
        .zip(&right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    value::encode_bool(difference == 0)
}

fn input_bytes(state: &NativeAgentState, encoded: i64) -> Option<Vec<u8>> {
    node_buffer::bytes(state, encoded).or_else(|| string(state, encoded).map(String::into_bytes))
}

fn string(state: &NativeAgentState, encoded: i64) -> Option<String> {
    state.string_owned(encoded)?.to_utf8()
}

fn intern(state: &mut NativeAgentState, text: String) -> Option<i64> {
    state.intern_text(text, value::TAG_STRING)
}

fn signed_integer(encoded: i64) -> Option<i64> {
    value::is_f64(encoded)
        .then(|| value::decode_f64(encoded))
        .filter(|number| number.is_finite() && number.fract() == 0.0)
        .filter(|number| *number >= i64::MIN as f64 && *number <= i64::MAX as f64)
        .map(|number| number as i64)
}

fn unsigned_integer(encoded: i64) -> Option<u64> {
    signed_integer(encoded)
        .filter(|number| *number >= 0)
        .map(|number| number as u64)
}

fn normalize_algorithm(algorithm: &str) -> String {
    algorithm
        .bytes()
        .filter(|byte| *byte != b'-')
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    exception(ctx, state, "TypeError", message)
}

fn range_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    exception(ctx, state, "RangeError", message)
}

fn exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    message: &str,
) -> i64 {
    modules::named_error_object(state, name, message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
