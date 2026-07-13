use std::cmp::Ordering;
use std::ops::Range;

/// ECMAScript 字符串的 runtime 内部表示：UTF-16 code units，可包含未配对 surrogate。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeString {
    units: Vec<u16>,
}

impl RuntimeString {
    pub(crate) fn empty() -> Self {
        Self { units: Vec::new() }
    }

    pub(crate) fn from_utf8_str(s: &str) -> Self {
        Self {
            units: s.encode_utf16().collect(),
        }
    }

    pub(crate) fn from_utf8_lossy(bytes: &[u8]) -> Self {
        Self::from_utf8_str(&String::from_utf8_lossy(bytes))
    }

    pub(crate) fn from_utf16_units(units: Vec<u16>) -> Self {
        Self { units }
    }

    pub(crate) fn from_utf16_code_unit(unit: u16) -> Self {
        Self { units: vec![unit] }
    }

    pub(crate) fn as_utf16_units(&self) -> &[u16] {
        &self.units
    }

    #[expect(
        dead_code,
        reason = "planned RuntimeString boundary API for future consumers"
    )]
    pub(crate) fn into_utf16_units(self) -> Vec<u16> {
        self.units
    }

    pub(crate) fn utf16_len(&self) -> usize {
        self.units.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    pub(crate) fn code_unit_at(&self, index: usize) -> Option<u16> {
        self.units.get(index).copied()
    }

    pub(crate) fn code_point_at(&self, index: usize) -> Option<u32> {
        let unit = self.code_unit_at(index)?;
        if is_high_surrogate(unit)
            && let Some(next) = self.code_unit_at(index + 1)
            && is_low_surrogate(next)
        {
            return Some(decode_surrogate_pair(unit, next));
        }
        Some(unit as u32)
    }

    pub(crate) fn slice_units(&self, range: Range<usize>) -> Self {
        Self::from_utf16_units(self.units[range].to_vec())
    }

    pub(crate) fn push_units_from(&mut self, other: &Self) {
        self.units.extend_from_slice(other.as_utf16_units());
    }

    pub(crate) fn repeat(&self, count: usize) -> Self {
        Self::from_utf16_units(self.units.repeat(count))
    }

    pub(crate) fn find_units(&self, needle: &Self, from: usize) -> Option<usize> {
        if needle.is_empty() {
            return Some(from.min(self.utf16_len()));
        }
        if from > self.utf16_len() || needle.utf16_len() > self.utf16_len() {
            return None;
        }
        self.units[from..]
            .windows(needle.utf16_len())
            .position(|window| window == needle.as_utf16_units())
            .map(|offset| from + offset)
    }

    pub(crate) fn rfind_units_before(&self, needle: &Self, end: usize) -> Option<usize> {
        let end = end.min(self.utf16_len());
        if needle.is_empty() {
            return Some(end);
        }
        if needle.utf16_len() > end {
            return None;
        }
        self.units[..end]
            .windows(needle.utf16_len())
            .rposition(|window| window == needle.as_utf16_units())
    }

    pub(crate) fn starts_with_units(&self, needle: &Self, from: usize) -> bool {
        let end = from.saturating_add(needle.utf16_len());
        self.units
            .get(from..end)
            .is_some_and(|units| units == needle.as_utf16_units())
    }

    pub(crate) fn ends_with_units(&self, needle: &Self, end: usize) -> bool {
        let end = end.min(self.utf16_len());
        let Some(start) = end.checked_sub(needle.utf16_len()) else {
            return false;
        };
        self.units
            .get(start..end)
            .is_some_and(|units| units == needle.as_utf16_units())
    }

    pub(crate) fn to_utf8(&self) -> Option<String> {
        let mut out = String::new();
        for item in std::char::decode_utf16(self.units.iter().copied()) {
            out.push(item.ok()?);
        }
        Some(out)
    }

    pub(crate) fn to_utf8_lossy(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }

    pub(crate) fn to_utf8_lossy_bytes(&self) -> Vec<u8> {
        self.to_utf8_lossy().into_bytes()
    }

    pub(crate) fn to_json_quoted(&self) -> String {
        let mut out = String::with_capacity(self.units.len() + 2);
        out.push('"');
        let mut i = 0usize;
        while i < self.units.len() {
            let unit = self.units[i];
            if is_high_surrogate(unit)
                && i + 1 < self.units.len()
                && is_low_surrogate(self.units[i + 1])
            {
                let cp = decode_surrogate_pair(unit, self.units[i + 1]);
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

    pub(crate) fn cmp_utf16(&self, other: &Self) -> Ordering {
        self.units.cmp(&other.units)
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
}
