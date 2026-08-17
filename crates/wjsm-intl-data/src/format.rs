//! 数字、日期、时区、历法、collation、复数与列表数据。

use icu::calendar::{AnyCalendar, AnyCalendarKind};
use icu::collator::Collator;
use icu::datetime::DateTimeFormatter;
use icu::datetime::fieldsets::M;
use icu::datetime::input::Date;
use icu::decimal::DecimalFormatter;
use icu::decimal::input::Decimal;
use icu::list::ListFormatter;
use icu::list::options::{ListFormatterOptions, ListLength};
use icu::locale::Locale;
use icu::plurals::PluralRules;
use icu::time::zone::IanaParser;

pub fn decimal_formatter(locale: &Locale) -> Result<DecimalFormatter, String> {
    DecimalFormatter::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn format_number(locale: &Locale, value: i32) -> Result<String, String> {
    let formatter = decimal_formatter(locale)?;
    Ok(formatter.format_to_string(&Decimal::from(value)))
}

pub fn format_month(locale: &Locale) -> Result<String, String> {
    let formatter = DateTimeFormatter::try_new(locale.into(), M::long()).map_err(err)?;
    let date = Date::try_new_iso(1970, 1, 11).map_err(err)?;
    Ok(formatter.format(&date).to_string())
}

pub fn collator(locale: &Locale) -> Result<icu::collator::CollatorBorrowed<'static>, String> {
    Collator::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn plural_rules(locale: &Locale) -> Result<PluralRules, String> {
    PluralRules::try_new(locale.into(), Default::default()).map_err(err)
}

pub fn format_and_list(locale: &Locale, items: &[&str]) -> Result<String, String> {
    let formatter = ListFormatter::try_new_and(
        locale.into(),
        ListFormatterOptions::default().with_length(ListLength::Wide),
    )
    .map_err(err)?;
    Ok(formatter.format(items.iter().copied()).to_string())
}

pub fn calendar(kind: AnyCalendarKind) -> AnyCalendar {
    AnyCalendar::new(kind)
}

pub fn parse_timezone(iana: &str) -> icu::time::TimeZone {
    IanaParser::new().parse(iana)
}

pub fn keep_format_data() {
    let _ = decimal_formatter(&icu::locale::locale!("en"));
    let _ = DateTimeFormatter::try_new(icu::locale::locale!("en").into(), M::long());
    let _ = collator(&icu::locale::locale!("en"));
    let _ = plural_rules(&icu::locale::locale!("en"));
    let _ = ListFormatter::try_new_and(icu::locale::locale!("en").into(), Default::default());
    let _ = calendar(AnyCalendarKind::Gregorian);
    let _ = calendar(AnyCalendarKind::Japanese);
    let _ = calendar(AnyCalendarKind::Chinese);
    let _ = calendar(AnyCalendarKind::Buddhist);
    let _ = calendar(AnyCalendarKind::HijriUmmAlQura);
    let _ = parse_timezone("America/New_York");
    let _ = parse_timezone("Asia/Shanghai");
    let _ = parse_timezone("Asia/Tokyo");
    let _ = parse_timezone("Europe/Berlin");
    let _ = parse_timezone("Asia/Bangkok");
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
