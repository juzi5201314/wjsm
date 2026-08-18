const collator = new Intl.Collator("de");
console.log(typeof collator.compare, collator.compare("a", "b") < 0);
console.log(collator.resolvedOptions().locale);

const number = new Intl.NumberFormat("de-DE");
console.log(number.format(1234.5));
const parts = number.formatToParts(1234.5);
console.log(parts.some((part) => part.type === "group" || part.type === "integer"));
console.log(number.formatRange(1, 2).includes("1"));

const date = new Date(Date.UTC(2024, 0, 15, 12, 0, 0));
const dtf = new Intl.DateTimeFormat("zh-CN", {
  timeZone: "UTC",
  year: "numeric",
  month: "numeric",
  day: "numeric",
});
console.log(dtf.resolvedOptions().timeZone, dtf.resolvedOptions().year);
console.log(String(dtf.format(date)).length > 0);
console.log(dtf.formatToParts(date).some((part) => part.type === "year"));

const plural = new Intl.PluralRules("en");
console.log(plural.select(1), plural.select(2));

const list = new Intl.ListFormat("en", { type: "conjunction" });
console.log(list.format(["a", "b", "c"]));

const relative = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
console.log(relative.format(-1, "day"));

const names = new Intl.DisplayNames("en", { type: "language" });
console.log(names.of("zh"));

const segmenter = new Intl.Segmenter("en", { granularity: "grapheme" });
let count = 0;
for (const item of segmenter.segment("ab")) {
  count += 1;
  console.log(item.segment, item.index);
}
console.log(count);
const words = new Intl.Segmenter("en", { granularity: "word" });
const hello = words.segment("hello world").containing(0);
console.log(hello.segment, hello.isWordLike);

try {
  new Intl.NumberFormat("en", { style: "currency" });
} catch (error) {
  console.log(error.name);
}
try {
  new Intl.DateTimeFormat("en", { dateStyle: "short", year: "numeric" });
} catch (error) {
  console.log(error.name);
}
