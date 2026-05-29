// replacer as function — documents current behavior.
// If not fully implemented, fixture records actual output (may differ from spec).
const data = { a: 1, b: 2, c: 3 };
const result = JSON.stringify(data, (key, value) => {
  if (key === "b") return undefined;
  return value;
});
console.log(result);
