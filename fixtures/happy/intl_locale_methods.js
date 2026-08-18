console.log("I".toLocaleLowerCase("tr"));
console.log("i".toLocaleUpperCase("tr"));
console.log("I".toLowerCase());
console.log("a".localeCompare("b", "en") < 0);
console.log((1234.5).toLocaleString("de-DE"));
console.log((1234n).toLocaleString("de-DE"));
const date = new Date(Date.UTC(2024, 0, 15, 12, 0, 0));
console.log(date.toLocaleDateString("en-US", { timeZone: "UTC" }).length > 0);
console.log(new Date(NaN).toLocaleString("en"));
const holes = [1, , 3];
console.log(holes.toLocaleString("en"));
const override = [1, 2];
override.toLocaleString = function () {
  return "overridden";
};
console.log(override.toLocaleString("de-DE"));
const nested = [
  {
    toLocaleString() {
      return "X";
    },
  },
  null,
  undefined,
  4,
];
console.log(nested.toLocaleString("en"));
console.log("e\u0301".normalize("NFC") === "\u00e9");
const fn = String.prototype.normalize;
console.log(typeof fn, fn.call("e\u0301", "NFC") === "\u00e9");
