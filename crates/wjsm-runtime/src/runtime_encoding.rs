use base64::Engine;
use base64::engine::general_purpose;

use crate::runtime_string::RuntimeString;
use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufferEncoding {
    Utf8,
    Utf16Le,
    Latin1,
    Ascii,
    Hex,
    Base64,
    Base64Url,
}

pub(crate) fn normalize_buffer_encoding(label: &str) -> Result<BufferEncoding, String> {
    let lower = label.trim().to_ascii_lowercase();
    match lower.as_str() {
        "utf8" | "utf-8" => Ok(BufferEncoding::Utf8),
        "utf16le" | "utf-16le" | "ucs2" | "ucs-2" => Ok(BufferEncoding::Utf16Le),
        "latin1" | "binary" => Ok(BufferEncoding::Latin1),
        "ascii" => Ok(BufferEncoding::Ascii),
        "hex" => Ok(BufferEncoding::Hex),
        "base64" => Ok(BufferEncoding::Base64),
        "base64url" => Ok(BufferEncoding::Base64Url),
        _ => Err(label.to_string()),
    }
}

pub(crate) fn encoding_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> Result<BufferEncoding, String> {
    if value::is_undefined(value_raw) {
        return Ok(BufferEncoding::Utf8);
    }
    let label = js_string_lossy(caller, value_raw);
    normalize_buffer_encoding(&label)
}

pub(crate) fn js_string_value(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
) -> RuntimeString {
    if value::is_string(value_raw) || value::is_runtime_string_handle(value_raw) {
        return get_string_value(caller, value_raw);
    }
    RuntimeString::from_utf8_str(&render_value(caller, value_raw).unwrap_or_default())
}

pub(crate) fn js_string_lossy(caller: &mut Caller<'_, RuntimeState>, value_raw: i64) -> String {
    js_string_value(caller, value_raw).to_utf8_lossy()
}

pub(crate) fn encode_js_string(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: i64,
    encoding: BufferEncoding,
) -> Result<Vec<u8>, String> {
    let string = js_string_value(caller, value_raw);
    encode_runtime_string(&string, encoding)
}

pub(crate) fn encode_runtime_string(
    string: &RuntimeString,
    encoding: BufferEncoding,
) -> Result<Vec<u8>, String> {
    Ok(match encoding {
        BufferEncoding::Utf8 => string.to_utf8_lossy().into_bytes(),
        BufferEncoding::Utf16Le => {
            let mut bytes = Vec::with_capacity(string.utf16_len() * 2);
            for unit in string.as_utf16_units() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        BufferEncoding::Latin1 => string
            .as_utf16_units()
            .iter()
            .map(|unit| (unit & 0x00ff) as u8)
            .collect(),
        BufferEncoding::Ascii => string
            .as_utf16_units()
            .iter()
            .map(|unit| (unit & 0x007f) as u8)
            .collect(),
        BufferEncoding::Hex => decode_hex_string(&string.to_utf8_lossy()),
        BufferEncoding::Base64 => decode_base64_string(&string.to_utf8_lossy(), false)?,
        BufferEncoding::Base64Url => decode_base64_string(&string.to_utf8_lossy(), true)?,
    })
}

pub(crate) fn decode_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8],
    encoding: BufferEncoding,
) -> i64 {
    match encoding {
        BufferEncoding::Utf8 => store_runtime_string(caller, RuntimeString::from_utf8_lossy(bytes)),
        BufferEncoding::Utf16Le => {
            let units = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            store_runtime_string(caller, RuntimeString::from_utf16_units(units))
        }
        BufferEncoding::Latin1 => {
            let units = bytes.iter().map(|byte| *byte as u16).collect::<Vec<_>>();
            store_runtime_string(caller, RuntimeString::from_utf16_units(units))
        }
        BufferEncoding::Ascii => {
            let units = bytes
                .iter()
                .map(|byte| (byte & 0x7f) as u16)
                .collect::<Vec<_>>();
            store_runtime_string(caller, RuntimeString::from_utf16_units(units))
        }
        BufferEncoding::Hex => store_runtime_string(caller, encode_hex(bytes)),
        BufferEncoding::Base64 => {
            store_runtime_string(caller, general_purpose::STANDARD.encode(bytes))
        }
        BufferEncoding::Base64Url => {
            store_runtime_string(caller, general_purpose::URL_SAFE_NO_PAD.encode(bytes))
        }
    }
}

pub(crate) fn decode_base64_string(input: &str, url_safe: bool) -> Result<Vec<u8>, String> {
    let mut normalized = String::with_capacity(input.len() + 3);
    for ch in input.chars() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        normalized.push(match ch {
            '-' => '+',
            '_' => '/',
            other => other,
        });
    }
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    let _ = url_safe;
    general_purpose::STANDARD
        .decode(normalized.as_bytes())
        .map_err(|e| e.to_string())
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_string(input: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut chars = input.bytes();
    while let Some(high) = chars.next() {
        let Some(low) = chars.next() else {
            break;
        };
        let Some(high) = hex_value(high) else {
            break;
        };
        let Some(low) = hex_value(low) else {
            break;
        };
        out.push((high << 4) | low);
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
