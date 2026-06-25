// ES §24.5.2: replacer this 必须是 holder（含属性所在的对象），非 value 本身。
const data = { a: 1, b: 2, c: 3 };
const result = JSON.stringify(data, function (key, value) {
  if (key === "b" && this[key] === 2) return undefined;
  return value;
});
console.log(result);

const nested = { outer: { inner: 1, drop: 2 } };
const nestedResult = JSON.stringify(nested, function (key, val) {
  if (key === "drop" && this[key] === 2) return undefined;
  return val;
});
console.log(nestedResult);