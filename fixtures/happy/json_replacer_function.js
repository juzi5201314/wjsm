// replacer function should omit object properties when it returns undefined.
const data = { a: 1, b: 2, c: 3 };
const result = JSON.stringify(data, (_key, value) => {
  if (value === 2) return undefined;
  return value;
});
console.log(result);
