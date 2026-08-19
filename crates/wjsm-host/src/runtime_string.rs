//! ECMAScript 字符串的 runtime 内部表示（后端无关）：rope + 惰性扁平化。
//!
//! V8 `ConsString` / JSC `RopeString` 路线：`+` / `slice` / `split` 快捷路径
//! 产出 rope 节点 O(1) 不扁平，仅在需要连续 `&[u16]` 时（哈希/查找/渲染）
//! 才一次性扁平化。节点以 `Arc` 共享，克隆 O(1) 不深拷。

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

const ROPE_FLATTEN_THRESHOLD: usize = 256;
const ROPE_CONCAT_SHORT_FLATTEN: usize = 64;

#[derive(Clone, Debug)]
enum RopeKind {
    Flat(Vec<u16>),
    Builder(Vec<u16>),
    Concat {
        children: Arc<(RuntimeString, RuntimeString)>,
        len: usize,
    },
    Slice {
        base: Arc<RuntimeString>,
        start: usize,
        end: usize,
        len: usize,
    },
    Repeat {
        base: Arc<RuntimeString>,
        count: usize,
        len: usize,
    },
}

#[derive(Clone, Debug)]
pub struct RuntimeString {
    kind: RopeKind,
    flat: OnceLock<Arc<[u16]>>,
}

impl Default for RuntimeString {
    fn default() -> Self {
        Self::empty()
    }
}

impl PartialEq for RuntimeString {
    fn eq(&self, other: &Self) -> bool {
        if self.utf16_len() != other.utf16_len() {
            return false;
        }
        if let (Some(a), Some(b)) = (self.try_flat_slice(), other.try_flat_slice()) {
            return a == b;
        }
        self.as_flat_slice() == other.as_flat_slice()
    }
}

impl Eq for RuntimeString {}

impl Borrow<[u16]> for RuntimeString {
    fn borrow(&self) -> &[u16] {
        self.as_flat_slice()
    }
}

impl Hash for RuntimeString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_flat_slice().hash(state);
    }
}

impl RuntimeString {
    pub fn empty() -> Self {
        Self {
            kind: RopeKind::Flat(Vec::new()),
            flat: OnceLock::from(Arc::from(Vec::<u16>::new().into_boxed_slice())),
        }
    }

    pub fn from_utf8_str(s: &str) -> Self {
        Self::from_utf16_units(s.encode_utf16().collect())
    }

    pub fn from_utf8_lossy(bytes: &[u8]) -> Self {
        Self::from_utf8_str(&String::from_utf8_lossy(bytes))
    }

    pub fn from_utf16_units(units: Vec<u16>) -> Self {
        let flat: Arc<[u16]> = Arc::from(units.into_boxed_slice());
        Self {
            kind: RopeKind::Flat(Vec::new()),
            flat: OnceLock::from(flat),
        }
    }

    fn from_owned_utf16_units(units: Vec<u16>) -> Self {
        Self {
            kind: RopeKind::Flat(units),
            flat: OnceLock::new(),
        }
    }

    pub fn from_utf16_code_unit(unit: u16) -> Self {
        Self::from_utf16_units(vec![unit])
    }
    pub fn as_flat_slice(&self) -> &[u16] {
        if let Some(flat) = self.try_flat_slice() {
            return flat;
        }
        self.flat
            .get_or_init(|| Arc::from(self.flatten_to_vec().into_boxed_slice()))
    }

    fn try_flat_slice(&self) -> Option<&[u16]> {
        self.flat
            .get()
            .map(AsRef::as_ref)
            .or_else(|| match &self.kind {
                RopeKind::Flat(units) => Some(units.as_slice()),
                _ => None,
            })
    }

    fn flatten_to_vec(&self) -> Vec<u16> {
        let len = self.utf16_len();
        let mut out = Vec::with_capacity(len);
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut Vec<u16>) {
        if let Some(flat) = self.try_flat_slice() {
            out.extend_from_slice(flat);
            return;
        }
        match &self.kind {
            RopeKind::Flat(v) => {
                if !v.is_empty() {
                    out.extend_from_slice(v);
                } else if let Some(flat) = self.flat.get() {
                    out.extend_from_slice(flat);
                }
            }
            RopeKind::Builder(v) => out.extend_from_slice(v),
            RopeKind::Concat { children, .. } => {
                children.0.write_into(out);
                children.1.write_into(out);
            }
            RopeKind::Slice {
                base, start, end, ..
            } => {
                write_slice_into(base, *start, *end, out);
            }
            RopeKind::Repeat { base, count, .. } => {
                if *count == 0 {
                    return;
                }
                if let Some(flat) = base.try_flat_slice() {
                    for _ in 0..*count {
                        out.extend_from_slice(flat);
                    }
                } else {
                    let flat = base.flatten_to_vec();
                    for _ in 0..*count {
                        out.extend_from_slice(&flat);
                    }
                }
            }
        }
    }

    pub fn as_utf16_units(&self) -> &[u16] {
        self.as_flat_slice()
    }

    pub fn into_utf16_units(self) -> Vec<u16> {
        self.as_flat_slice().to_vec()
    }

    pub fn flattened_units(&self) -> Arc<[u16]> {
        Arc::clone(
            self.flat
                .get_or_init(|| Arc::from(self.flatten_to_vec().into_boxed_slice())),
        )
    }

    pub fn utf16_len(&self) -> usize {
        match &self.kind {
            RopeKind::Flat(v) => {
                if !v.is_empty() {
                    v.len()
                } else {
                    self.flat.get().map_or(0, |f| f.len())
                }
            }
            RopeKind::Builder(v) => v.len(),
            RopeKind::Concat { len, .. } => *len,
            RopeKind::Slice { len, .. } => *len,
            RopeKind::Repeat { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.utf16_len() == 0
    }

    pub fn code_unit_at(&self, index: usize) -> Option<u16> {
        if index >= self.utf16_len() {
            return None;
        }
        match &self.kind {
            RopeKind::Flat(v) => {
                if !v.is_empty() {
                    return v.get(index).copied();
                }
                self.flat.get()?.get(index).copied()
            }
            RopeKind::Builder(v) => v.get(index).copied(),
            RopeKind::Concat { children, .. } => {
                let left_len = children.0.utf16_len();
                if index < left_len {
                    children.0.code_unit_at(index)
                } else {
                    children.1.code_unit_at(index - left_len)
                }
            }
            RopeKind::Slice { base, start, .. } => base.code_unit_at(start + index),
            RopeKind::Repeat { base, .. } => {
                let bl = base.utf16_len();
                if bl == 0 {
                    return None;
                }
                base.code_unit_at(index % bl)
            }
        }
    }

    pub fn code_point_at(&self, index: usize) -> Option<u32> {
        let unit = self.code_unit_at(index)?;
        if is_high_surrogate(unit)
            && let Some(next) = self.code_unit_at(index + 1)
            && is_low_surrogate(next)
        {
            return Some(decode_surrogate_pair(unit, next));
        }
        Some(unit as u32)
    }

    pub fn slice_units(&self, range: Range<usize>) -> Self {
        let len = self.utf16_len();
        let start = range.start.min(len);
        let end = range.end.min(len);
        if start >= end {
            return Self::empty();
        }
        if start == 0 && end == len {
            return self.clone();
        }
        if let Some(flat) = self.try_flat_slice() {
            return Self::from_owned_utf16_units(flat[start..end].to_vec());
        }
        let slice_len = end - start;
        if slice_len <= ROPE_FLATTEN_THRESHOLD {
            let mut out = Vec::with_capacity(slice_len);
            write_slice_into(self, start, end, &mut out);
            return Self::from_owned_utf16_units(out);
        }
        Self {
            kind: RopeKind::Slice {
                base: Arc::new(self.clone()),
                start,
                end,
                len: slice_len,
            },
            flat: OnceLock::new(),
        }
    }

    pub fn push_units_from(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = other.clone();
            return;
        }
        let new = Self::concat(self.clone(), other.clone());
        *self = new;
    }

    pub fn concat(left: Self, right: Self) -> Self {
        if left.is_empty() {
            return right;
        }
        if right.is_empty() {
            return left;
        }
        let total = left.utf16_len() + right.utf16_len();
        if total <= ROPE_CONCAT_SHORT_FLATTEN
            && left.try_flat_slice().is_some()
            && right.try_flat_slice().is_some()
        {
            let mut out = Vec::with_capacity(total);
            out.extend_from_slice(left.try_flat_slice().unwrap());
            out.extend_from_slice(right.try_flat_slice().unwrap());
            return Self::from_utf16_units(out);
        }
        Self {
            kind: RopeKind::Concat {
                children: Arc::new((left, right)),
                len: total,
            },
            flat: OnceLock::new(),
        }
    }

    /// 从右向左组合一组已完成 ToString 的片段，使末尾短扁平串先合并，
    /// 避免长累加器阻止相邻短片段触发扁平化。
    pub fn concat_many(parts: Vec<Self>) -> Self {
        parts
            .into_iter()
            .rev()
            .reduce(|right, left| Self::concat(left, right))
            .unwrap_or_default()
    }

    pub fn repeat(&self, count: usize) -> Self {
        if count == 0 || self.is_empty() {
            return Self::empty();
        }
        if count == 1 {
            return self.clone();
        }
        let len = self.utf16_len().checked_mul(count).unwrap_or(usize::MAX);
        if len <= ROPE_FLATTEN_THRESHOLD && self.try_flat_slice().is_some() {
            let flat = self.try_flat_slice().unwrap();
            let mut out = Vec::with_capacity(len);
            for _ in 0..count {
                out.extend_from_slice(flat);
            }
            return Self::from_utf16_units(out);
        }
        Self {
            kind: RopeKind::Repeat {
                base: Arc::new(self.clone()),
                count,
                len,
            },
            flat: OnceLock::new(),
        }
    }

    pub fn find_units(&self, needle: &Self, from: usize) -> Option<usize> {
        if needle.is_empty() {
            return Some(from.min(self.utf16_len()));
        }
        if from > self.utf16_len() || needle.utf16_len() > self.utf16_len() {
            return None;
        }
        let hay = self.as_flat_slice();
        let ndl = needle.as_flat_slice();
        hay[from..]
            .windows(ndl.len())
            .position(|window| window == ndl)
            .map(|offset| from + offset)
    }

    pub fn rfind_units_before(&self, needle: &Self, end: usize) -> Option<usize> {
        let end = end.min(self.utf16_len());
        if needle.is_empty() {
            return Some(end);
        }
        if needle.utf16_len() > end {
            return None;
        }
        let hay = self.as_flat_slice();
        let ndl = needle.as_flat_slice();
        hay[..end]
            .windows(ndl.len())
            .rposition(|window| window == ndl)
    }

    pub fn starts_with_units(&self, needle: &Self, from: usize) -> bool {
        let nlen = needle.utf16_len();
        if from + nlen > self.utf16_len() {
            return false;
        }
        let hay = self.as_flat_slice();
        let ndl = needle.as_flat_slice();
        hay[from..from + nlen] == ndl[..]
    }

    pub fn ends_with_units(&self, needle: &Self, end: usize) -> bool {
        let end = end.min(self.utf16_len());
        let Some(start) = end.checked_sub(needle.utf16_len()) else {
            return false;
        };
        let hay = self.as_flat_slice();
        let ndl = needle.as_flat_slice();
        hay[start..end] == ndl[..]
    }

    pub fn to_utf8(&self) -> Option<String> {
        let flat = self.as_flat_slice();
        let mut out = String::new();
        for item in std::char::decode_utf16(flat.iter().copied()) {
            out.push(item.ok()?);
        }
        Some(out)
    }

    pub fn to_utf8_lossy(&self) -> String {
        let flat = self.as_flat_slice();
        String::from_utf16_lossy(&flat)
    }

    pub fn to_utf8_lossy_bytes(&self) -> Vec<u8> {
        self.to_utf8_lossy().into_bytes()
    }

    pub fn to_json_quoted(&self) -> String {
        let flat = self.as_flat_slice();
        let mut out = String::with_capacity(flat.len() + 2);
        out.push('"');
        let mut i = 0usize;
        while i < flat.len() {
            let unit = flat[i];
            if is_high_surrogate(unit) && i + 1 < flat.len() && is_low_surrogate(flat[i + 1]) {
                let cp = decode_surrogate_pair(unit, flat[i + 1]);
                push_json_char(&mut out, char::from_u32(cp).expect("valid surrogate pair"));
                i += 2;
                continue;
            }
            if is_high_surrogate(unit) || is_low_surrogate(unit) {
                push_json_u_escape(&mut out, unit);
            } else {
                push_json_char(
                    &mut out,
                    char::from_u32(unit as u32).expect("valid BMP scalar"),
                );
            }
            i += 1;
        }
        out.push('"');
        out
    }

    pub fn cmp_utf16(&self, other: &Self) -> Ordering {
        self.as_flat_slice().cmp(other.as_flat_slice())
    }

    /// 创建仅供编译器证明不逃逸的局部累加器使用的可变缓冲区。
    pub fn builder(capacity: usize) -> Self {
        Self {
            kind: RopeKind::Builder(Vec::with_capacity(capacity)),
            flat: OnceLock::new(),
        }
    }

    pub fn append_builder(&mut self, part: &Self) -> bool {
        let RopeKind::Builder(units) = &mut self.kind else {
            return false;
        };
        part.write_into(units);
        true
    }

    pub fn append_builder_utf8(&mut self, text: &str) -> bool {
        let RopeKind::Builder(units) = &mut self.kind else {
            return false;
        };
        units.extend(text.encode_utf16());
        true
    }

    pub fn append_builder_number(&mut self, number: f64) -> bool {
        let RopeKind::Builder(units) = &mut self.kind else {
            return false;
        };
        append_number_units(units, number)
    }

    pub fn append_builder_string_number(&mut self, part: &Self, number: f64) -> bool {
        let RopeKind::Builder(units) = &mut self.kind else {
            return false;
        };
        part.write_into(units);
        append_number_units(units, number)
    }

    pub fn finish_builder(&mut self) -> bool {
        let RopeKind::Builder(units) =
            std::mem::replace(&mut self.kind, RopeKind::Flat(Vec::new()))
        else {
            return false;
        };
        self.kind = RopeKind::Flat(units);
        true
    }

    pub fn is_builder(&self) -> bool {
        matches!(self.kind, RopeKind::Builder(_))
    }

    pub fn is_flat(&self) -> bool {
        matches!(self.kind, RopeKind::Flat(_)) || self.flat.get().is_some()
    }
    /// 估算本值本次创建独占的宿主堆字节，用于把字符串分配反馈给 GC pacing。
    /// 共享的 rope 子串与已有扁平缓冲不会重复计费。
    pub fn estimated_owned_bytes(&self) -> usize {
        let flat_bytes = self
            .flat
            .get()
            .filter(|flat| Arc::strong_count(flat) == 1)
            .map_or(0, |flat| flat.len().saturating_mul(size_of::<u16>()));
        let node_bytes = match &self.kind {
            RopeKind::Flat(units) | RopeKind::Builder(units) => {
                units.capacity().saturating_mul(size_of::<u16>())
            }
            RopeKind::Concat { children, .. } if Arc::strong_count(children) == 1 => {
                size_of::<(RuntimeString, RuntimeString)>()
            }
            RopeKind::Slice { base, .. } | RopeKind::Repeat { base, .. }
                if Arc::strong_count(base) == 1 =>
            {
                size_of::<RuntimeString>()
            }
            RopeKind::Concat { .. } | RopeKind::Slice { .. } | RopeKind::Repeat { .. } => 0,
        };
        flat_bytes.saturating_add(node_bytes)
    }
}

impl From<&str> for RuntimeString {
    fn from(value: &str) -> Self {
        Self::from_utf8_str(value)
    }
}

impl From<String> for RuntimeString {
    fn from(value: String) -> Self {
        Self::from_utf8_str(&value)
    }
}

impl From<Vec<u16>> for RuntimeString {
    fn from(value: Vec<u16>) -> Self {
        Self::from_utf16_units(value)
    }
}

struct Utf16Writer<'a>(&'a mut Vec<u16>);

impl std::fmt::Write for Utf16Writer<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend(text.encode_utf16());
        Ok(())
    }
}
fn safe_integer(number: f64) -> Option<i64> {
    if !(-9_007_199_254_740_991.0..=9_007_199_254_740_991.0).contains(&number) {
        return None;
    }
    // 范围检查保证转换不饱和；随后 round-trip 只接受能被 f64 精确表示的整数。
    let integer = number as i64;
    (integer as f64 == number).then_some(integer)
}

fn append_number_units(units: &mut Vec<u16>, number: f64) -> bool {
    use std::fmt::Write as _;

    let text = if number.is_nan() {
        Some("NaN")
    } else if number == f64::INFINITY {
        Some("Infinity")
    } else if number == f64::NEG_INFINITY {
        Some("-Infinity")
    } else {
        None
    };
    if let Some(text) = text {
        units.extend(text.encode_utf16());
        return true;
    }
    if number == 0.0 {
        units.push(u16::from(b'0'));
        return true;
    }
    if let Some(integer) = safe_integer(number) {
        append_safe_integer(units, integer);
        return true;
    }
    write!(Utf16Writer(units), "{number}").is_ok()
}

fn append_safe_integer(units: &mut Vec<u16>, number: i64) {
    let mut digits = [0_u16; 20];
    let mut cursor = digits.len();
    let mut magnitude = number.unsigned_abs();
    while magnitude != 0 {
        cursor -= 1;
        digits[cursor] = u16::from(b'0') + (magnitude % 10) as u16;
        magnitude /= 10;
    }
    if number < 0 {
        cursor -= 1;
        digits[cursor] = u16::from(b'-');
    }
    units.extend_from_slice(&digits[cursor..]);
}

fn write_slice_into(base: &RuntimeString, start: usize, end: usize, out: &mut Vec<u16>) {
    if let Some(flat) = base.try_flat_slice() {
        out.extend_from_slice(&flat[start..end]);
        return;
    }
    match &base.kind {
        RopeKind::Flat(v) => {
            if !v.is_empty() {
                out.extend_from_slice(&v[start..end]);
            } else if let Some(flat) = base.flat.get() {
                out.extend_from_slice(&flat[start..end]);
            }
        }
        RopeKind::Builder(v) => out.extend_from_slice(&v[start..end]),
        RopeKind::Concat { children, .. } => {
            let left_len = children.0.utf16_len();
            if end <= left_len {
                write_slice_into(&children.0, start, end, out);
            } else if start >= left_len {
                write_slice_into(&children.1, start - left_len, end - left_len, out);
            } else {
                write_slice_into(&children.0, start, left_len, out);
                write_slice_into(&children.1, 0, end - left_len, out);
            }
        }
        RopeKind::Slice {
            base: inner,
            start: s,
            ..
        } => {
            write_slice_into(inner, s + start, s + end, out);
        }
        RopeKind::Repeat {
            base: inner,
            count: _,
            ..
        } => {
            let bl = inner.utf16_len();
            if bl == 0 {
                return;
            }
            for idx in start..end {
                if let Some(u) = inner.code_unit_at(idx % bl) {
                    out.push(u);
                }
            }
        }
    }
}

fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

fn decode_surrogate_pair(high: u16, low: u16) -> u32 {
    0x10000 + (((high as u32 - 0xD800) << 10) | (low as u32 - 0xDC00))
}

fn push_json_char(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0C}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{00}'..='\u{1F}' => push_json_u_escape(out, ch as u16),
        _ => out.push(ch),
    }
}

fn push_json_u_escape(out: &mut String, unit: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    out.push(HEX[((unit >> 12) & 0xF) as usize] as char);
    out.push(HEX[((unit >> 8) & 0xF) as usize] as char);
    out.push(HEX[((unit >> 4) & 0xF) as usize] as char);
    out.push(HEX[(unit & 0xF) as usize] as char);
}

#[cfg(test)]
mod tests {
    use super::RuntimeString;

    #[test]
    fn lone_surrogate_roundtrips_units() {
        let string = RuntimeString::from_utf16_code_unit(0xD800);
        assert_eq!(string.utf16_len(), 1);
        assert_eq!(string.code_unit_at(0), Some(0xD800));
    }

    #[test]
    fn lone_surrogate_json_quote() {
        let string = RuntimeString::from_utf16_code_unit(0xD800);
        assert_eq!(string.to_json_quoted(), "\"\\ud800\"");
    }

    #[test]
    fn valid_pair_code_point_at() {
        let string = RuntimeString::from_utf16_units(vec![0xD83D, 0xDE00]);
        assert_eq!(string.code_point_at(0), Some(0x1F600));
        assert_eq!(string.code_point_at(1), Some(0xDE00));
    }

    #[test]
    fn unit_slice_does_not_require_utf8() {
        let string = RuntimeString::from_utf16_units(vec![0x41, 0xD800, 0x42]);
        let slice = string.slice_units(1..2);
        assert_eq!(slice.utf16_len(), 1);
        assert_eq!(slice.code_unit_at(0), Some(0xD800));
    }

    #[test]
    fn concat_is_rope_not_flat() {
        let a = RuntimeString::from_utf8_str(&"a".repeat(200));
        let b = RuntimeString::from_utf8_str(&"b".repeat(200));
        let c = RuntimeString::concat(a, b);
        assert_eq!(c.utf16_len(), 400);
        assert!(!c.is_flat());
        assert_eq!(c.code_unit_at(0), Some(b'a' as u16));
        assert_eq!(c.code_unit_at(399), Some(b'b' as u16));
    }

    #[test]
    fn finish_builder_preserves_contents() {
        let mut string = RuntimeString::builder(4);
        assert!(string.append_builder_utf8("abcd"));
        assert!(string.finish_builder());
        assert_eq!(string.as_utf16_units(), &[0x61, 0x62, 0x63, 0x64]);
    }
    #[test]
    fn slice_of_rope_preserves_units() {
        let a = RuntimeString::from_utf16_units((0..1000).map(|i| (i % 256) as u16).collect());
        let b =
            RuntimeString::from_utf16_units((0..1000).map(|i| (255 - (i % 256)) as u16).collect());
        let c = RuntimeString::concat(a, b);
        let s = c.slice_units(900..1100);
        assert_eq!(s.utf16_len(), 200);
        assert_eq!(s.as_flat_slice().len(), 200);
    }
}
