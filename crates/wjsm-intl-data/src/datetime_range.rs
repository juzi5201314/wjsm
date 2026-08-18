//! `PartitionDateTimeRangePattern`：按最大差异字段折叠区间。

use crate::datetime::OwnedDateTimeFormatter;
use crate::format::FormatPart;

const RANGE_SEP: &str = " – ";

impl OwnedDateTimeFormatter {
    pub fn format_range_millis(&self, start: f64, end: f64) -> Result<String, String> {
        Ok(self
            .format_range_parts_millis(start, end)?
            .into_iter()
            .map(|part| part.value)
            .collect())
    }

    pub fn format_range_parts_millis(
        &self,
        start: f64,
        end: f64,
    ) -> Result<Vec<FormatPart>, String> {
        let start_parts = self.format_parts_millis(start)?;
        let end_parts = self.format_parts_millis(end)?;
        if same_values(&start_parts, &end_parts) {
            return Ok(mark_source(start_parts, "shared"));
        }
        if can_collapse(&start_parts, &end_parts) {
            return Ok(collapse_range(start_parts, end_parts));
        }
        Ok(concat_range(start_parts, end_parts))
    }
}

fn same_values(start: &[FormatPart], end: &[FormatPart]) -> bool {
    start.len() == end.len()
        && start
            .iter()
            .zip(end)
            .all(|(left, right)| left.type_name == right.type_name && left.value == right.value)
}

fn can_collapse(start: &[FormatPart], end: &[FormatPart]) -> bool {
    start.len() == end.len()
        && month_is_named(start)
        && field(start, "year") == field(end, "year")
        && field(start, "relatedYear") == field(end, "relatedYear")
}

fn month_is_named(parts: &[FormatPart]) -> bool {
    parts
        .iter()
        .any(|part| part.type_name == "month" && part.value.chars().any(|ch| ch.is_alphabetic()))
}

fn field<'a>(parts: &'a [FormatPart], name: &str) -> Option<&'a str> {
    parts
        .iter()
        .find(|part| part.type_name == name)
        .map(|part| part.value.as_str())
}

fn collapse_range(start: Vec<FormatPart>, end: Vec<FormatPart>) -> Vec<FormatPart> {
    let first = start
        .iter()
        .zip(&end)
        .position(|(left, right)| left.type_name != "literal" && left.value != right.value);
    let last = start
        .iter()
        .zip(&end)
        .rposition(|(left, right)| left.type_name != "literal" && left.value != right.value);
    let (Some(first), Some(last)) = (first, last) else {
        return mark_source(start, "shared");
    };
    let mut out = Vec::with_capacity(start.len() + 2);
    out.extend(mark_source(start[..first].to_vec(), "shared"));
    out.extend(mark_source(start[first..=last].to_vec(), "startRange"));
    out.push(separator());
    out.extend(mark_source(end[first..=last].to_vec(), "endRange"));
    out.extend(mark_source(start[last + 1..].to_vec(), "shared"));
    out
}

fn concat_range(start: Vec<FormatPart>, end: Vec<FormatPart>) -> Vec<FormatPart> {
    let mut out = mark_source(start, "startRange");
    out.push(separator());
    out.extend(mark_source(end, "endRange"));
    out
}

fn mark_source(mut parts: Vec<FormatPart>, source: &str) -> Vec<FormatPart> {
    for part in &mut parts {
        part.source = Some(source.into());
    }
    parts
}

fn separator() -> FormatPart {
    FormatPart {
        type_name: "literal".into(),
        value: RANGE_SEP.into(),
        source: Some("shared".into()),
        unit: None,
    }
}
