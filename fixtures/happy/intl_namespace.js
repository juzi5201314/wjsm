console.log(typeof Intl);
console.log(Object.prototype.toString.call(Intl));
console.log(Intl.getCanonicalLocales.length, Intl.supportedValuesOf.length);
console.log(Object.getOwnPropertyDescriptor(Intl, "Collator").enumerable);
console.log(typeof Intl.Locale, typeof Intl.Collator, typeof Intl.NumberFormat);
console.log(typeof Intl.DateTimeFormat, typeof Intl.PluralRules, typeof Intl.ListFormat);
console.log(typeof Intl.RelativeTimeFormat, typeof Intl.DisplayNames, typeof Intl.Segmenter);
console.log(typeof Intl.DurationFormat);
console.log(JSON.stringify(Intl.getCanonicalLocales("DE-de")));
console.log(JSON.stringify(Intl.getCanonicalLocales(["en-US", "en-us", "zh-CN"])));
console.log(Intl.supportedValuesOf("calendar").includes("gregory"));
console.log(Intl.supportedValuesOf("currency").includes("USD"));
console.log(JSON.stringify(Intl.getCanonicalLocales(1)));
try {
  Intl.getCanonicalLocales(["*"]);
} catch (error) {
  console.log(error.name);
}
try {
  Intl.supportedValuesOf("hourCycle");
} catch (error) {
  console.log(error.name);
}
const tainted = ["en"];
Object.defineProperty(Object.prototype, "localeMatcher", {
  get() {
    console.log("taint");
    return "lookup";
  },
  configurable: true,
});
console.log(JSON.stringify(Intl.Collator.supportedLocalesOf(tainted)));
delete Object.prototype.localeMatcher;
