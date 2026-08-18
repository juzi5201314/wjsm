//! NumberFormat 编号系统数字、货币小数位与 NaN 文案。

/// ISO 4217 小数位；未列出的货币默认 2。
pub fn currency_digits(code: &str) -> u32 {
    match code {
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" => 4,
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        _ => 2,
    }
}

pub fn locale_nan(locale: &str) -> &'static str {
    if is_zh_hant(locale) {
        "\u{975e}\u{6578}\u{503c}"
    } else if locale_language(locale) == "zh" {
        "\u{975e}\u{6570}\u{503c}"
    } else {
        "NaN"
    }
}

pub fn is_numbering_digit(ch: char, numbering_system: &str) -> bool {
    numbering_digits(numbering_system)
        .unwrap_or("0123456789")
        .chars()
        .any(|digit| digit == ch)
        || ch.is_ascii_digit()
        || (!ch.is_ascii() && ch.is_numeric())
}

pub fn substitute_digits(text: &str, numbering_system: &str) -> String {
    let Some(digits) = numbering_digits(numbering_system) else {
        return text.to_owned();
    };
    if digits == "0123456789" {
        return text.to_owned();
    }
    let map: Vec<char> = digits.chars().collect();
    text.chars()
        .map(|ch| {
            if ch.is_ascii_digit() {
                map[(ch as u8 - b'0') as usize]
            } else {
                ch
            }
        })
        .collect()
}

fn locale_language(locale: &str) -> &str {
    locale.split(['-', '_']).next().unwrap_or(locale)
}

fn is_zh_hant(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    lower.starts_with("zh-hant")
        || lower.starts_with("zh-tw")
        || lower.starts_with("zh-hk")
        || lower.starts_with("zh-mo")
}

fn numbering_digits(system: &str) -> Option<&'static str> {
    Some(match system {
        "latn" => "0123456789",
        other => return extra_digits(other),
    })
}

fn extra_digits(system: &str) -> Option<&'static str> {
    DIGITS
        .iter()
        .find(|(name, _)| *name == system)
        .map(|(_, digits)| *digits)
}

const DIGITS: &[(&str, &str)] = &[
    (
        "adlm",
        "\u{1E950}\u{1E951}\u{1E952}\u{1E953}\u{1E954}\u{1E955}\u{1E956}\u{1E957}\u{1E958}\u{1E959}",
    ),
    (
        "ahom",
        "\u{11730}\u{11731}\u{11732}\u{11733}\u{11734}\u{11735}\u{11736}\u{11737}\u{11738}\u{11739}",
    ),
    (
        "arab",
        "\u{0660}\u{0661}\u{0662}\u{0663}\u{0664}\u{0665}\u{0666}\u{0667}\u{0668}\u{0669}",
    ),
    (
        "arabext",
        "\u{06F0}\u{06F1}\u{06F2}\u{06F3}\u{06F4}\u{06F5}\u{06F6}\u{06F7}\u{06F8}\u{06F9}",
    ),
    (
        "bali",
        "\u{1B50}\u{1B51}\u{1B52}\u{1B53}\u{1B54}\u{1B55}\u{1B56}\u{1B57}\u{1B58}\u{1B59}",
    ),
    (
        "beng",
        "\u{09E6}\u{09E7}\u{09E8}\u{09E9}\u{09EA}\u{09EB}\u{09EC}\u{09ED}\u{09EE}\u{09EF}",
    ),
    (
        "bhks",
        "\u{11C50}\u{11C51}\u{11C52}\u{11C53}\u{11C54}\u{11C55}\u{11C56}\u{11C57}\u{11C58}\u{11C59}",
    ),
    (
        "brah",
        "\u{11066}\u{11067}\u{11068}\u{11069}\u{1106A}\u{1106B}\u{1106C}\u{1106D}\u{1106E}\u{1106F}",
    ),
    (
        "cakm",
        "\u{11136}\u{11137}\u{11138}\u{11139}\u{1113A}\u{1113B}\u{1113C}\u{1113D}\u{1113E}\u{1113F}",
    ),
    (
        "cham",
        "\u{AA50}\u{AA51}\u{AA52}\u{AA53}\u{AA54}\u{AA55}\u{AA56}\u{AA57}\u{AA58}\u{AA59}",
    ),
    (
        "deva",
        "\u{0966}\u{0967}\u{0968}\u{0969}\u{096A}\u{096B}\u{096C}\u{096D}\u{096E}\u{096F}",
    ),
    (
        "diak",
        "\u{11950}\u{11951}\u{11952}\u{11953}\u{11954}\u{11955}\u{11956}\u{11957}\u{11958}\u{11959}",
    ),
    (
        "fullwide",
        "\u{FF10}\u{FF11}\u{FF12}\u{FF13}\u{FF14}\u{FF15}\u{FF16}\u{FF17}\u{FF18}\u{FF19}",
    ),
    (
        "gara",
        "\u{10D40}\u{10D41}\u{10D42}\u{10D43}\u{10D44}\u{10D45}\u{10D46}\u{10D47}\u{10D48}\u{10D49}",
    ),
    (
        "gong",
        "\u{11DA0}\u{11DA1}\u{11DA2}\u{11DA3}\u{11DA4}\u{11DA5}\u{11DA6}\u{11DA7}\u{11DA8}\u{11DA9}",
    ),
    (
        "gonm",
        "\u{11D50}\u{11D51}\u{11D52}\u{11D53}\u{11D54}\u{11D55}\u{11D56}\u{11D57}\u{11D58}\u{11D59}",
    ),
    (
        "gujr",
        "\u{0AE6}\u{0AE7}\u{0AE8}\u{0AE9}\u{0AEA}\u{0AEB}\u{0AEC}\u{0AED}\u{0AEE}\u{0AEF}",
    ),
    (
        "gukh",
        "\u{16130}\u{16131}\u{16132}\u{16133}\u{16134}\u{16135}\u{16136}\u{16137}\u{16138}\u{16139}",
    ),
    (
        "guru",
        "\u{0A66}\u{0A67}\u{0A68}\u{0A69}\u{0A6A}\u{0A6B}\u{0A6C}\u{0A6D}\u{0A6E}\u{0A6F}",
    ),
    (
        "hanidec",
        "\u{3007}\u{4E00}\u{4E8C}\u{4E09}\u{56DB}\u{4E94}\u{516D}\u{4E03}\u{516B}\u{4E5D}",
    ),
    (
        "hmng",
        "\u{16B50}\u{16B51}\u{16B52}\u{16B53}\u{16B54}\u{16B55}\u{16B56}\u{16B57}\u{16B58}\u{16B59}",
    ),
    (
        "hmnp",
        "\u{1E140}\u{1E141}\u{1E142}\u{1E143}\u{1E144}\u{1E145}\u{1E146}\u{1E147}\u{1E148}\u{1E149}",
    ),
    (
        "java",
        "\u{A9D0}\u{A9D1}\u{A9D2}\u{A9D3}\u{A9D4}\u{A9D5}\u{A9D6}\u{A9D7}\u{A9D8}\u{A9D9}",
    ),
    (
        "kali",
        "\u{A900}\u{A901}\u{A902}\u{A903}\u{A904}\u{A905}\u{A906}\u{A907}\u{A908}\u{A909}",
    ),
    (
        "kawi",
        "\u{11F50}\u{11F51}\u{11F52}\u{11F53}\u{11F54}\u{11F55}\u{11F56}\u{11F57}\u{11F58}\u{11F59}",
    ),
    (
        "khmr",
        "\u{17E0}\u{17E1}\u{17E2}\u{17E3}\u{17E4}\u{17E5}\u{17E6}\u{17E7}\u{17E8}\u{17E9}",
    ),
    (
        "knda",
        "\u{0CE6}\u{0CE7}\u{0CE8}\u{0CE9}\u{0CEA}\u{0CEB}\u{0CEC}\u{0CED}\u{0CEE}\u{0CEF}",
    ),
    (
        "krai",
        "\u{16D70}\u{16D71}\u{16D72}\u{16D73}\u{16D74}\u{16D75}\u{16D76}\u{16D77}\u{16D78}\u{16D79}",
    ),
    (
        "lana",
        "\u{1A80}\u{1A81}\u{1A82}\u{1A83}\u{1A84}\u{1A85}\u{1A86}\u{1A87}\u{1A88}\u{1A89}",
    ),
    (
        "lanatham",
        "\u{1A90}\u{1A91}\u{1A92}\u{1A93}\u{1A94}\u{1A95}\u{1A96}\u{1A97}\u{1A98}\u{1A99}",
    ),
    (
        "laoo",
        "\u{0ED0}\u{0ED1}\u{0ED2}\u{0ED3}\u{0ED4}\u{0ED5}\u{0ED6}\u{0ED7}\u{0ED8}\u{0ED9}",
    ),
    (
        "lepc",
        "\u{1C40}\u{1C41}\u{1C42}\u{1C43}\u{1C44}\u{1C45}\u{1C46}\u{1C47}\u{1C48}\u{1C49}",
    ),
    (
        "limb",
        "\u{1946}\u{1947}\u{1948}\u{1949}\u{194A}\u{194B}\u{194C}\u{194D}\u{194E}\u{194F}",
    ),
    (
        "mathbold",
        "\u{1D7CE}\u{1D7CF}\u{1D7D0}\u{1D7D1}\u{1D7D2}\u{1D7D3}\u{1D7D4}\u{1D7D5}\u{1D7D6}\u{1D7D7}",
    ),
    (
        "mathdbl",
        "\u{1D7D8}\u{1D7D9}\u{1D7DA}\u{1D7DB}\u{1D7DC}\u{1D7DD}\u{1D7DE}\u{1D7DF}\u{1D7E0}\u{1D7E1}",
    ),
    (
        "mathmono",
        "\u{1D7F6}\u{1D7F7}\u{1D7F8}\u{1D7F9}\u{1D7FA}\u{1D7FB}\u{1D7FC}\u{1D7FD}\u{1D7FE}\u{1D7FF}",
    ),
    (
        "mathsanb",
        "\u{1D7EC}\u{1D7ED}\u{1D7EE}\u{1D7EF}\u{1D7F0}\u{1D7F1}\u{1D7F2}\u{1D7F3}\u{1D7F4}\u{1D7F5}",
    ),
    (
        "mathsans",
        "\u{1D7E2}\u{1D7E3}\u{1D7E4}\u{1D7E5}\u{1D7E6}\u{1D7E7}\u{1D7E8}\u{1D7E9}\u{1D7EA}\u{1D7EB}",
    ),
    (
        "mlym",
        "\u{0D66}\u{0D67}\u{0D68}\u{0D69}\u{0D6A}\u{0D6B}\u{0D6C}\u{0D6D}\u{0D6E}\u{0D6F}",
    ),
    (
        "modi",
        "\u{11650}\u{11651}\u{11652}\u{11653}\u{11654}\u{11655}\u{11656}\u{11657}\u{11658}\u{11659}",
    ),
    (
        "mong",
        "\u{1810}\u{1811}\u{1812}\u{1813}\u{1814}\u{1815}\u{1816}\u{1817}\u{1818}\u{1819}",
    ),
    (
        "mroo",
        "\u{16A60}\u{16A61}\u{16A62}\u{16A63}\u{16A64}\u{16A65}\u{16A66}\u{16A67}\u{16A68}\u{16A69}",
    ),
    (
        "mtei",
        "\u{ABF0}\u{ABF1}\u{ABF2}\u{ABF3}\u{ABF4}\u{ABF5}\u{ABF6}\u{ABF7}\u{ABF8}\u{ABF9}",
    ),
    (
        "mymr",
        "\u{1040}\u{1041}\u{1042}\u{1043}\u{1044}\u{1045}\u{1046}\u{1047}\u{1048}\u{1049}",
    ),
    (
        "mymrepka",
        "\u{116DA}\u{116DB}\u{116DC}\u{116DD}\u{116DE}\u{116DF}\u{116E0}\u{116E1}\u{116E2}\u{116E3}",
    ),
    (
        "mymrpao",
        "\u{116D0}\u{116D1}\u{116D2}\u{116D3}\u{116D4}\u{116D5}\u{116D6}\u{116D7}\u{116D8}\u{116D9}",
    ),
    (
        "mymrshan",
        "\u{1090}\u{1091}\u{1092}\u{1093}\u{1094}\u{1095}\u{1096}\u{1097}\u{1098}\u{1099}",
    ),
    (
        "mymrtlng",
        "\u{A9F0}\u{A9F1}\u{A9F2}\u{A9F3}\u{A9F4}\u{A9F5}\u{A9F6}\u{A9F7}\u{A9F8}\u{A9F9}",
    ),
    (
        "nagm",
        "\u{1E4F0}\u{1E4F1}\u{1E4F2}\u{1E4F3}\u{1E4F4}\u{1E4F5}\u{1E4F6}\u{1E4F7}\u{1E4F8}\u{1E4F9}",
    ),
    (
        "newa",
        "\u{11450}\u{11451}\u{11452}\u{11453}\u{11454}\u{11455}\u{11456}\u{11457}\u{11458}\u{11459}",
    ),
    (
        "nkoo",
        "\u{07C0}\u{07C1}\u{07C2}\u{07C3}\u{07C4}\u{07C5}\u{07C6}\u{07C7}\u{07C8}\u{07C9}",
    ),
    (
        "olck",
        "\u{1C50}\u{1C51}\u{1C52}\u{1C53}\u{1C54}\u{1C55}\u{1C56}\u{1C57}\u{1C58}\u{1C59}",
    ),
    (
        "onao",
        "\u{1E5F1}\u{1E5F2}\u{1E5F3}\u{1E5F4}\u{1E5F5}\u{1E5F6}\u{1E5F7}\u{1E5F8}\u{1E5F9}\u{1E5FA}",
    ),
    (
        "orya",
        "\u{0B66}\u{0B67}\u{0B68}\u{0B69}\u{0B6A}\u{0B6B}\u{0B6C}\u{0B6D}\u{0B6E}\u{0B6F}",
    ),
    (
        "osma",
        "\u{104A0}\u{104A1}\u{104A2}\u{104A3}\u{104A4}\u{104A5}\u{104A6}\u{104A7}\u{104A8}\u{104A9}",
    ),
    (
        "outlined",
        "\u{1CCF0}\u{1CCF1}\u{1CCF2}\u{1CCF3}\u{1CCF4}\u{1CCF5}\u{1CCF6}\u{1CCF7}\u{1CCF8}\u{1CCF9}",
    ),
    (
        "rohg",
        "\u{10D30}\u{10D31}\u{10D32}\u{10D33}\u{10D34}\u{10D35}\u{10D36}\u{10D37}\u{10D38}\u{10D39}",
    ),
    (
        "saur",
        "\u{A8D0}\u{A8D1}\u{A8D2}\u{A8D3}\u{A8D4}\u{A8D5}\u{A8D6}\u{A8D7}\u{A8D8}\u{A8D9}",
    ),
    (
        "segment",
        "\u{1FBF0}\u{1FBF1}\u{1FBF2}\u{1FBF3}\u{1FBF4}\u{1FBF5}\u{1FBF6}\u{1FBF7}\u{1FBF8}\u{1FBF9}",
    ),
    (
        "shrd",
        "\u{111D0}\u{111D1}\u{111D2}\u{111D3}\u{111D4}\u{111D5}\u{111D6}\u{111D7}\u{111D8}\u{111D9}",
    ),
    (
        "sind",
        "\u{112F0}\u{112F1}\u{112F2}\u{112F3}\u{112F4}\u{112F5}\u{112F6}\u{112F7}\u{112F8}\u{112F9}",
    ),
    (
        "sinh",
        "\u{0DE6}\u{0DE7}\u{0DE8}\u{0DE9}\u{0DEA}\u{0DEB}\u{0DEC}\u{0DED}\u{0DEE}\u{0DEF}",
    ),
    (
        "sora",
        "\u{110F0}\u{110F1}\u{110F2}\u{110F3}\u{110F4}\u{110F5}\u{110F6}\u{110F7}\u{110F8}\u{110F9}",
    ),
    (
        "sund",
        "\u{1BB0}\u{1BB1}\u{1BB2}\u{1BB3}\u{1BB4}\u{1BB5}\u{1BB6}\u{1BB7}\u{1BB8}\u{1BB9}",
    ),
    (
        "sunu",
        "\u{11BF0}\u{11BF1}\u{11BF2}\u{11BF3}\u{11BF4}\u{11BF5}\u{11BF6}\u{11BF7}\u{11BF8}\u{11BF9}",
    ),
    (
        "takr",
        "\u{116C0}\u{116C1}\u{116C2}\u{116C3}\u{116C4}\u{116C5}\u{116C6}\u{116C7}\u{116C8}\u{116C9}",
    ),
    (
        "talu",
        "\u{19D0}\u{19D1}\u{19D2}\u{19D3}\u{19D4}\u{19D5}\u{19D6}\u{19D7}\u{19D8}\u{19D9}",
    ),
    (
        "tamldec",
        "\u{0BE6}\u{0BE7}\u{0BE8}\u{0BE9}\u{0BEA}\u{0BEB}\u{0BEC}\u{0BED}\u{0BEE}\u{0BEF}",
    ),
    (
        "telu",
        "\u{0C66}\u{0C67}\u{0C68}\u{0C69}\u{0C6A}\u{0C6B}\u{0C6C}\u{0C6D}\u{0C6E}\u{0C6F}",
    ),
    (
        "thai",
        "\u{0E50}\u{0E51}\u{0E52}\u{0E53}\u{0E54}\u{0E55}\u{0E56}\u{0E57}\u{0E58}\u{0E59}",
    ),
    (
        "tibt",
        "\u{0F20}\u{0F21}\u{0F22}\u{0F23}\u{0F24}\u{0F25}\u{0F26}\u{0F27}\u{0F28}\u{0F29}",
    ),
    (
        "tirh",
        "\u{114D0}\u{114D1}\u{114D2}\u{114D3}\u{114D4}\u{114D5}\u{114D6}\u{114D7}\u{114D8}\u{114D9}",
    ),
    (
        "tnsa",
        "\u{16AC0}\u{16AC1}\u{16AC2}\u{16AC3}\u{16AC4}\u{16AC5}\u{16AC6}\u{16AC7}\u{16AC8}\u{16AC9}",
    ),
    (
        "tols",
        "\u{11DE0}\u{11DE1}\u{11DE2}\u{11DE3}\u{11DE4}\u{11DE5}\u{11DE6}\u{11DE7}\u{11DE8}\u{11DE9}",
    ),
    (
        "vaii",
        "\u{A620}\u{A621}\u{A622}\u{A623}\u{A624}\u{A625}\u{A626}\u{A627}\u{A628}\u{A629}",
    ),
    (
        "wara",
        "\u{118E0}\u{118E1}\u{118E2}\u{118E3}\u{118E4}\u{118E5}\u{118E6}\u{118E7}\u{118E8}\u{118E9}",
    ),
    (
        "wcho",
        "\u{1E2F0}\u{1E2F1}\u{1E2F2}\u{1E2F3}\u{1E2F4}\u{1E2F5}\u{1E2F6}\u{1E2F7}\u{1E2F8}\u{1E2F9}",
    ),
];

#[cfg(test)]
mod tests {
    use super::{currency_digits, locale_nan, substitute_digits};

    #[test]
    fn jpy_has_zero_fraction_digits() {
        assert_eq!(currency_digits("JPY"), 0);
        assert_eq!(currency_digits("USD"), 2);
        assert_eq!(currency_digits("KWD"), 3);
        assert_eq!(currency_digits("CLF"), 4);
    }

    #[test]
    fn substitutes_ahom_digits() {
        assert_eq!(substitute_digits("0", "ahom"), "\u{11730}");
    }

    #[test]
    fn zh_tw_nan() {
        assert_eq!(locale_nan("zh-TW"), "\u{975e}\u{6578}\u{503c}");
        assert_eq!(locale_nan("en-US"), "NaN");
    }
}
