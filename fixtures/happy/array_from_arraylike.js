// Array.from 对没有 @@iterator 的普通 array-like 使用 LengthOfArrayLike。
const source = { "0": "a", "1": "b", "2": "ignored", length: 2.9 };
const indexes = [];
const result = Array.from(source, (value, index) => {
  indexes.push(index);
  return value + index;
});
console.log(result.join("-"));
console.log(indexes.join(","));
