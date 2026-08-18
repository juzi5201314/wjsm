const locale = new Intl.Locale("de-DE", { calendar: "gregory", hourCycle: "h23" });
console.log(locale.toString());
console.log(locale.language, locale.region, locale.baseName);
console.log(locale.calendar, locale.hourCycle);
console.log(locale.maximize().toString());
console.log(new Intl.Locale("zh-Hans-CN").minimize().toString());
console.log(locale.getCalendars().includes("gregory"));
console.log(locale.getTextInfo().direction);
try {
  Intl.Locale("de");
} catch (error) {
  console.log(error.name);
}
try {
  new Intl.Locale("*");
} catch (error) {
  console.log(error.name);
}
