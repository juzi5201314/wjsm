//! 按 grapheme 计算终端显示列：East Asian Width + Extended Pictographic。

use std::sync::OnceLock;

use wjsm_intl_data::icu::properties::props::{
    EastAsianWidth, ExtendedPictographic, GraphemeExtend,
};
use wjsm_intl_data::icu::properties::{
    CodePointMapData, CodePointMapDataBorrowed, CodePointSetData, CodePointSetDataBorrowed,
};
use wjsm_intl_data::{OwnedSegmenter, SegmentGranularity};

struct WidthTables {
    grapheme_extend: CodePointSetDataBorrowed<'static>,
    pictographic: CodePointSetDataBorrowed<'static>,
    east_asian: CodePointMapDataBorrowed<'static, EastAsianWidth>,
}

fn width_tables() -> &'static WidthTables {
    static TABLES: OnceLock<WidthTables> = OnceLock::new();
    TABLES.get_or_init(|| WidthTables {
        grapheme_extend: CodePointSetData::new::<GraphemeExtend>(),
        pictographic: CodePointSetData::new::<ExtendedPictographic>(),
        east_asian: CodePointMapData::<EastAsianWidth>::new(),
    })
}

/// `text[..end]` 的显示列；`end` 必须落在 UTF-8 边界。
pub(super) fn prefix_width(text: &str, end: usize) -> usize {
    display_width(&text[..end])
}

pub(super) fn display_width(text: &str) -> usize {
    grapheme_slices(text).into_iter().map(grapheme_width).sum()
}

fn grapheme_slices(text: &str) -> Vec<&str> {
    let bounds = OwnedSegmenter::new(SegmentGranularity::Grapheme).break_offsets(text);
    bounds
        .windows(2)
        .map(|pair| &text[pair[0]..pair[1]])
        .collect()
}

fn grapheme_width(cluster: &str) -> usize {
    if cluster.chars().all(is_zero_width) {
        return 0;
    }
    if cluster.chars().any(is_wide) {
        return 2;
    }
    1
}

fn is_zero_width(ch: char) -> bool {
    matches!(ch, '\u{200d}' | '\u{fe00}'..='\u{fe0f}')
        || ch.is_control()
        || width_tables().grapheme_extend.contains(ch)
}

fn is_wide(ch: char) -> bool {
    let eaw = width_tables().east_asian.get(ch);
    eaw == EastAsianWidth::Wide
        || eaw == EastAsianWidth::Fullwidth
        || width_tables().pictographic.contains(ch)
}

#[cfg(test)]
mod tests {
    use super::{display_width, grapheme_slices};

    #[test]
    fn ascii_cjk_combining_and_emoji_widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("中"), 2);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("e\u{0301}"), 1);
        assert_eq!(display_width("👋"), 2);
        assert_eq!(display_width("👨‍👩‍👧"), 2);
    }

    #[test]
    fn grapheme_steps_match_owned_segmenter() {
        assert_eq!(grapheme_slices("你好"), ["你", "好"]);
        assert_eq!(grapheme_slices("e\u{0301}"), ["e\u{0301}"]);
        assert_eq!(grapheme_slices("👨‍👩‍👧"), ["👨‍👩‍👧"]);
    }
}
