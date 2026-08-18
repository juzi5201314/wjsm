//! ECMA-402 `Intl.NumberFormat` 的 ICU 格式化。

use std::str::FromStr;

use icu::decimal::input::{Decimal, SignDisplay};
use icu::decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu::decimal::{CompactDecimalFormatter, DecimalFormatter};
use icu::experimental::dimension::currency::CurrencyCode;
use icu::experimental::dimension::currency::formatter::CurrencyFormatter;
use icu::experimental::dimension::currency::long_formatter::LongCurrencyFormatter;
use icu::experimental::dimension::currency::options::{CurrencyFormatterOptions, Width};
use icu::experimental::dimension::percent::formatter::PercentFormatter;
use icu::experimental::dimension::units::formatter::UnitsFormatter;
use icu::experimental::dimension::units::options::{UnitsFormatterOptions, Width as UnitWidth};
use icu::locale::Locale;
use tinystr::TinyAsciiStr;

use crate::format::FormatPart;
use crate::number_parts::{
    SplitCtx, apply_currency_code, build_range, pad_integer_digits, separators, split_formatted,
    trim_fraction_zeros,
};
use crate::number_round::apply_rounding;
use crate::number_symbols::{locale_nan, substitute_digits};

#[derive(Clone, Debug)]
pub struct NumberFormatSpec {
    pub locale: String,
    pub numbering_system: String,
    pub style: String,
    pub currency: Option<String>,
    pub currency_display: String,
    pub currency_sign: String,
    pub unit: Option<String>,
    pub unit_display: String,
    pub notation: String,
    pub compact_display: String,
    pub sign_display: String,
    pub use_grouping: String,
    pub minimum_integer_digits: u32,
    pub minimum_fraction_digits: u32,
    pub maximum_fraction_digits: u32,
    pub minimum_significant_digits: Option<u32>,
    pub maximum_significant_digits: Option<u32>,
    pub rounding_mode: String,
    pub rounding_increment: u32,
    pub rounding_priority: String,
    pub trailing_zero_display: String,
}

pub struct OwnedNumberFormatter {
    spec: NumberFormatSpec,
    decimal: DecimalFormatter,
    percent: Option<PercentFormatter<DecimalFormatter>>,
    currency: Option<CurrencyFormatter>,
    currency_long: Option<LongCurrencyFormatter>,
    compact: Option<CompactDecimalFormatter>,
    unit: Option<UnitsFormatter>,
}

impl OwnedNumberFormatter {
    pub fn try_new(spec: NumberFormatSpec) -> Result<Self, String> {
        let locale = format_locale(&spec.locale, &spec.numbering_system)?;
        let mut options = DecimalFormatterOptions::default();
        options.grouping_strategy = Some(grouping_strategy(&spec.use_grouping));
        let decimal = DecimalFormatter::try_new((&locale).into(), options).map_err(err)?;
        let percent = if spec.style == "percent" {
            Some(
                PercentFormatter::try_new_with_decimal_formatter(
                    (&locale).into(),
                    decimal.clone(),
                    Default::default(),
                )
                .map_err(err)?,
            )
        } else {
            None
        };
        let (currency, currency_long) = if spec.style == "currency" {
            currency_formatters(&spec, &locale)?
        } else {
            (None, None)
        };
        let compact = if spec.notation == "compact" {
            Some(if spec.compact_display == "long" {
                CompactDecimalFormatter::try_new_long((&locale).into(), options.into())
                    .map_err(err)?
            } else {
                CompactDecimalFormatter::try_new_short((&locale).into(), options.into())
                    .map_err(err)?
            })
        } else {
            None
        };
        let unit = if spec.style == "unit" {
            spec.unit
                .as_deref()
                .and_then(|unit| unit_formatter(&locale, unit, &spec.unit_display).ok())
        } else {
            None
        };
        Ok(Self {
            spec,
            decimal,
            percent,
            currency,
            currency_long,
            compact,
            unit,
        })
    }

    pub fn format_str(&self, raw: &str) -> Result<String, String> {
        Ok(self.format_kind(classify(raw)?)?.0)
    }

    pub fn format_parts_str(&self, raw: &str) -> Result<Vec<FormatPart>, String> {
        Ok(self.format_kind(classify(raw)?)?.1)
    }

    pub fn format_range_str(&self, start: &str, end: &str) -> Result<String, String> {
        Ok(self.format_range_kind(start, end)?.0)
    }

    pub fn format_range_parts_str(
        &self,
        start: &str,
        end: &str,
    ) -> Result<Vec<FormatPart>, String> {
        Ok(self.format_range_kind(start, end)?.1)
    }

    fn format_range_kind(
        &self,
        start: &str,
        end: &str,
    ) -> Result<(String, Vec<FormatPart>), String> {
        let (start_text, start_parts) = self.format_kind(classify(start)?)?;
        let (end_text, end_parts) = self.format_kind(classify(end)?)?;
        Ok(build_range(
            &self.spec,
            &start_text,
            start_parts,
            &end_text,
            end_parts,
        ))
    }

    fn format_kind(&self, kind: NumberKind) -> Result<(String, Vec<FormatPart>), String> {
        match kind {
            NumberKind::Nan => Ok(localize(nan_parts(&self.spec), &self.spec)),
            NumberKind::Infinity { negative } => {
                Ok(localize(infinity_parts(negative, &self.spec), &self.spec))
            }
            NumberKind::Finite(decimal) => {
                if matches!(self.spec.notation.as_str(), "scientific" | "engineering") {
                    return Ok(localize(
                        self.format_scientific(decimal, self.spec.notation == "engineering"),
                        &self.spec,
                    ));
                }
                let text = self.format_finite(decimal)?;
                let text = substitute_digits(&text, &self.spec.numbering_system);
                let (group, decimal_sep) = separators(&self.decimal, &self.spec.numbering_system);
                let parts = split_formatted(
                    &text,
                    &SplitCtx {
                        spec: &self.spec,
                        group,
                        decimal: decimal_sep,
                    },
                );
                Ok((text, parts))
            }
        }
    }

    fn format_finite(&self, mut decimal: Decimal) -> Result<String, String> {
        if self.spec.style == "percent" {
            decimal.multiply_pow10(2);
            decimal.trim_start();
        }
        let negative = matches!(decimal.sign(), fixed_decimal::Sign::Negative);
        let use_sig = if self.spec.notation == "compact" {
            false
        } else {
            apply_rounding(&mut decimal, &self.spec)
        };
        let accounting = uses_accounting(&self.spec, negative, &decimal);
        if accounting {
            apply_sign(&mut decimal, "never");
        } else {
            apply_sign(&mut decimal, &self.spec.sign_display);
        }
        if self.spec.trailing_zero_display == "stripIfInteger" {
            decimal.trim_end_if_integer();
        }
        let mut text = self.render_finite(&decimal)?;
        if self.spec.currency_display == "code"
            && let Some(code) = self.spec.currency.as_deref()
        {
            text = apply_currency_code(&text, code);
        }
        if accounting && negative && !text.starts_with('(') {
            text = format!("({text})");
        }
        if self.spec.minimum_integer_digits > 1 {
            text = pad_integer_digits(
                &text,
                self.spec.minimum_integer_digits,
                &self.spec.numbering_system,
            );
        }
        let (_, decimal_sep) = separators(&self.decimal, &self.spec.numbering_system);
        Ok(trim_fraction_zeros(
            &text,
            &decimal_sep,
            self.spec.minimum_fraction_digits,
            &self.spec.numbering_system,
            use_sig,
        ))
    }

    fn render_finite(&self, decimal: &Decimal) -> Result<String, String> {
        if let Some(compact) = &self.compact {
            return Ok(compact.format(decimal).to_string());
        }
        if let Some(long) = &self.currency_long {
            return Ok(long.format_fixed_decimal(decimal).to_string());
        }
        if let Some(currency) = &self.currency {
            let code = currency_code(self.spec.currency.as_deref().unwrap_or("USD"))?;
            return Ok(currency.format_fixed_decimal(decimal, &code).to_string());
        }
        if let Some(percent) = &self.percent {
            return Ok(percent.format(decimal).to_string());
        }
        if let Some(unit) = &self.unit {
            return Ok(unit.format_fixed_decimal(decimal).to_string());
        }
        let rendered = self.decimal.format_to_string(decimal);
        if self.spec.style == "unit" {
            let unit = self.spec.unit.as_deref().unwrap_or("");
            if !unit.is_empty() {
                return Ok(format!("{rendered} {unit}"));
            }
        }
        Ok(rendered)
    }

    fn format_scientific(&self, decimal: Decimal, engineering: bool) -> (String, Vec<FormatPart>) {
        if decimal.is_zero() {
            let mantissa = self.decimal.format_to_string(&decimal);
            return (
                format!("{mantissa}E0"),
                vec![
                    part("integer", &mantissa),
                    part("exponentSeparator", "E"),
                    part("exponentInteger", "0"),
                ],
            );
        }
        let magnitude = decimal.nonzero_magnitude_start();
        let exponent = if engineering {
            magnitude.div_euclid(3) * 3
        } else {
            magnitude
        };
        let mut mantissa = decimal;
        mantissa.multiply_pow10(-exponent);
        mantissa.trim_start();
        apply_rounding(&mut mantissa, &self.spec);
        mantissa.trim_start();
        apply_sign(&mut mantissa, &self.spec.sign_display);
        let rendered = locale_scientific_mantissa(&self.decimal, &mantissa);
        let text = format!("{rendered}E{exponent}");
        let mut parts = mantissa_parts(&rendered);
        parts.push(part("exponentSeparator", "E"));
        if exponent < 0 {
            parts.push(part("exponentMinusSign", "-"));
            parts.push(part("exponentInteger", &(-exponent).to_string()));
        } else {
            parts.push(part("exponentInteger", &exponent.to_string()));
        }
        (text, parts)
    }
}

enum NumberKind {
    Nan,
    Infinity { negative: bool },
    Finite(Decimal),
}

fn classify(raw: &str) -> Result<NumberKind, String> {
    match raw {
        "NaN" => Ok(NumberKind::Nan),
        "Infinity" => Ok(NumberKind::Infinity { negative: false }),
        "-Infinity" => Ok(NumberKind::Infinity { negative: true }),
        other => Ok(NumberKind::Finite(Decimal::from_str(other).map_err(err)?)),
    }
}

fn nan_parts(spec: &NumberFormatSpec) -> (String, Vec<FormatPart>) {
    let nan = locale_nan(&spec.locale);
    if spec.sign_display == "always" {
        (
            format!("+{nan}"),
            vec![part("plusSign", "+"), part("nan", nan)],
        )
    } else {
        (nan.into(), vec![part("nan", nan)])
    }
}

fn infinity_parts(negative: bool, spec: &NumberFormatSpec) -> (String, Vec<FormatPart>) {
    let show_minus = negative
        && matches!(
            spec.sign_display.as_str(),
            "auto" | "always" | "exceptZero" | "negative"
        );
    let show_plus = !negative && matches!(spec.sign_display.as_str(), "always" | "exceptZero");
    let (text, sign_part) = if show_minus {
        ("-∞", Some(part("minusSign", "-")))
    } else if show_plus {
        ("+∞", Some(part("plusSign", "+")))
    } else {
        ("∞", None)
    };
    let mut parts = Vec::new();
    if let Some(sign) = sign_part {
        parts.push(sign);
    }
    parts.push(part("infinity", "∞"));
    (text.into(), parts)
}

fn apply_sign(decimal: &mut Decimal, sign_display: &str) {
    let display = match sign_display {
        "always" => SignDisplay::Always,
        "never" => SignDisplay::Never,
        "exceptZero" => SignDisplay::ExceptZero,
        "negative" => SignDisplay::Negative,
        _ => SignDisplay::Auto,
    };
    decimal.apply_sign_display(display);
}

fn currency_formatters(
    spec: &NumberFormatSpec,
    locale: &Locale,
) -> Result<(Option<CurrencyFormatter>, Option<LongCurrencyFormatter>), String> {
    if spec.currency_display == "name" {
        let code = currency_code(spec.currency.as_deref().unwrap_or("USD"))?;
        let long = LongCurrencyFormatter::try_new(locale.into(), &code).map_err(err)?;
        return Ok((None, Some(long)));
    }
    let mut options = CurrencyFormatterOptions::default();
    options.width = if spec.currency_display == "narrowSymbol" {
        Width::Narrow
    } else {
        Width::Short
    };
    let formatter = CurrencyFormatter::try_new(locale.into(), options).map_err(err)?;
    Ok((Some(formatter), None))
}

fn currency_code(code: &str) -> Result<CurrencyCode, String> {
    let upper = code.to_ascii_uppercase();
    Ok(CurrencyCode(
        TinyAsciiStr::<3>::try_from_str(&upper).map_err(err)?,
    ))
}

fn format_locale(locale: &str, numbering_system: &str) -> Result<Locale, String> {
    let tag =
        if numbering_system.is_empty() || numbering_system == "latn" || locale.contains("-u-nu-") {
            locale.to_owned()
        } else {
            format!("{locale}-u-nu-{numbering_system}")
        };
    Locale::try_from_str(&tag).map_err(err)
}

fn uses_accounting(spec: &NumberFormatSpec, negative: bool, decimal: &Decimal) -> bool {
    if spec.style != "currency" || spec.currency_sign != "accounting" || !negative {
        return false;
    }
    if !accounting_uses_parens(&spec.locale) {
        return false;
    }
    match spec.sign_display.as_str() {
        "never" => false,
        "exceptZero" if decimal.is_zero() => false,
        "negative" if decimal.is_zero() => false,
        _ => true,
    }
}

fn accounting_uses_parens(locale: &str) -> bool {
    matches!(
        locale.split(['-', '_']).next().unwrap_or(locale),
        "en" | "ja" | "ko" | "zh"
    )
}

fn mantissa_parts(rendered: &str) -> Vec<FormatPart> {
    let mut parts = Vec::new();
    let mut rest = rendered;
    if let Some(stripped) = rest.strip_prefix('+') {
        parts.push(part("plusSign", "+"));
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('-') {
        parts.push(part("minusSign", "-"));
        rest = stripped;
    }
    if let Some((integer, fraction)) = rest.split_once(['.', ',']) {
        let sep = if rest.contains(',') { "," } else { "." };
        parts.push(part("integer", integer));
        parts.push(part("decimal", sep));
        parts.push(part("fraction", fraction));
    } else {
        parts.push(part("integer", rest));
    }
    parts
}

fn locale_scientific_mantissa(decimal: &DecimalFormatter, mantissa: &Decimal) -> String {
    let ascii = mantissa.to_string();
    let sample =
        decimal.format_to_string(&Decimal::from_str("1.1").unwrap_or_else(|_| Decimal::from(1)));
    let separator = sample
        .chars()
        .find(|ch| !ch.is_ascii_digit() && *ch != '+' && *ch != '-')
        .unwrap_or('.');
    ascii.replace('.', &separator.to_string())
}

fn unit_formatter(
    locale: &Locale,
    unit: &str,
    unit_display: &str,
) -> Result<UnitsFormatter, String> {
    let mut options = UnitsFormatterOptions::default();
    options.width = match unit_display {
        "long" => UnitWidth::Long,
        "narrow" => UnitWidth::Narrow,
        _ => UnitWidth::Short,
    };
    UnitsFormatter::try_new(locale.into(), unit, options).map_err(err)
}

fn localize(
    result: (String, Vec<FormatPart>),
    spec: &NumberFormatSpec,
) -> (String, Vec<FormatPart>) {
    let (text, parts) = result;
    let text = substitute_digits(&text, &spec.numbering_system);
    let parts = parts
        .into_iter()
        .map(|part| FormatPart {
            type_name: part.type_name,
            value: substitute_digits(&part.value, &spec.numbering_system),
            source: part.source,
            unit: part.unit,
        })
        .collect();
    (text, parts)
}

fn grouping_strategy(use_grouping: &str) -> GroupingStrategy {
    match use_grouping {
        "false" => GroupingStrategy::Never,
        "always" => GroupingStrategy::Always,
        "min2" => GroupingStrategy::Min2,
        _ => GroupingStrategy::Auto,
    }
}

fn part(type_name: &str, value: &str) -> FormatPart {
    FormatPart {
        type_name: type_name.into(),
        value: value.into(),
        source: None,
        unit: None,
    }
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{NumberFormatSpec, OwnedNumberFormatter};
    use icu::decimal::input::Decimal;
    use std::str::FromStr;

    fn spec(style: &str) -> NumberFormatSpec {
        NumberFormatSpec {
            locale: "en-US".into(),
            numbering_system: "latn".into(),
            style: style.into(),
            currency: (style == "currency").then(|| "USD".into()),
            currency_display: "symbol".into(),
            currency_sign: "standard".into(),
            unit: None,
            unit_display: "short".into(),
            notation: "standard".into(),
            compact_display: "short".into(),
            sign_display: "auto".into(),
            use_grouping: "auto".into(),
            minimum_integer_digits: 1,
            minimum_fraction_digits: if style == "currency" { 2 } else { 0 },
            maximum_fraction_digits: if style == "currency" { 2 } else { 3 },
            minimum_significant_digits: None,
            maximum_significant_digits: None,
            rounding_mode: "halfExpand".into(),
            rounding_increment: 1,
            rounding_priority: "auto".into(),
            trailing_zero_display: "auto".into(),
        }
    }

    #[test]
    fn percent_multiply_is_25() {
        let mut decimal = Decimal::from_str("0.25").expect("parse");
        decimal.multiply_pow10(2);
        decimal.trim_start();
        assert_eq!(decimal.to_string(), "25");
    }

    #[test]
    fn scientific_keeps_small_mantissa() {
        let mut scientific = spec("decimal");
        scientific.notation = "scientific".into();
        let formatter = OwnedNumberFormatter::try_new(scientific).expect("sci");
        assert_eq!(formatter.format_str("0.000345").expect("fmt"), "3.45E-4");
        let mut engineering = spec("decimal");
        engineering.notation = "engineering".into();
        let formatter = OwnedNumberFormatter::try_new(engineering).expect("eng");
        assert_eq!(formatter.format_str("0.000345").expect("fmt"), "345E-6");
    }

    #[test]
    fn fraction_min_does_not_keep_max_zeros() {
        let mut decimal_spec = spec("decimal");
        decimal_spec.minimum_fraction_digits = 1;
        decimal_spec.maximum_fraction_digits = 3;
        let formatter = OwnedNumberFormatter::try_new(decimal_spec).expect("fmt");
        assert_eq!(formatter.format_str("123").expect("fmt"), "123.0");
    }

    #[test]
    fn grouping_keeps_thousands() {
        let formatter = OwnedNumberFormatter::try_new(spec("decimal")).expect("fmt");
        assert_eq!(formatter.format_str("1000").expect("fmt"), "1,000");
    }

    #[test]
    fn formats_percent_and_currency() {
        let mut percent_spec = spec("percent");
        percent_spec.maximum_fraction_digits = 0;
        let percent = OwnedNumberFormatter::try_new(percent_spec).expect("percent");
        assert_eq!(percent.format_str("0.25").expect("fmt"), "25%");
        let currency = OwnedNumberFormatter::try_new(spec("currency")).expect("currency");
        let rendered = currency.format_str("-987").expect("fmt");
        assert!(rendered.contains("987"), "{rendered}");
        assert!(
            rendered.contains('$') || rendered.contains("USD"),
            "{rendered}"
        );
    }
}
