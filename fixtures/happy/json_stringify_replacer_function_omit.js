const result = JSON.stringify({ a: 1, b: 2, c: 3 }, (_key, value) => {
  if (value === 2) return undefined;
  return value;
});
console.log(result);
