//! rustc 风格的源码诊断格式化（行号、列号、源码片段、caret）。

use swc_core::common::Spanned;
use swc_core::common::sync::Lrc;
use swc_core::common::{BytePos, FileName, SourceMap};
use swc_core::ecma::parser::error::Error as ParseError;

/// 将字节偏移格式化为带行/列与源码片段的诊断文本。
pub fn format_byte_diagnostic(
    filename: &str,
    source: &str,
    label: &str,
    start: u32,
    end: u32,
) -> String {
    let cm: Lrc<SourceMap> = Default::default();
    let _fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );
    let start_pos = BytePos(start);
    let end_pos = BytePos(end.max(start));
    let loc = cm.lookup_char_pos(start_pos);
    let line = loc.line;
    let col = loc.col.0 + 1;
    let file_display = filename;

    let mut out = format!("error: {label}\n --> {file_display}:{line}:{col}\n");

    let line_index = line.saturating_sub(1);
    if let Some(line_text) = source.lines().nth(line_index) {
        let gutter = format!("{line} | ");
        out.push_str(&gutter);
        out.push_str(line_text);
        out.push('\n');
        out.push_str(&format!(
            "{} | ",
            " ".repeat(gutter.len().saturating_sub(3))
        ));
        let caret_col = loc.col.0.min(line_text.len());
        for _ in 0..caret_col {
            out.push(' ');
        }
        let span_len = if end > start {
            let end_loc = cm.lookup_char_pos(end_pos);
            if end_loc.line == line {
                (end_loc.col.0.saturating_sub(loc.col.0)).max(1)
            } else {
                line_text.len().saturating_sub(caret_col).max(1)
            }
        } else {
            1
        };
        for _ in 0..span_len {
            out.push('^');
        }
        out.push('\n');
    }

    out
}

/// 将 SWC 解析错误格式化为带位置信息的诊断文本。
pub fn format_parse_error(
    _cm: &Lrc<SourceMap>,
    filename: &str,
    source: &str,
    err: ParseError,
) -> String {
    let span = err.span();
    let message = err.kind().msg().into_owned();
    format_byte_diagnostic(filename, source, &message, span.lo.0, span.hi.0)
}
