// Test global builtins: undefined, NaN, Infinity
console.log(undefined);
console.log(NaN);
console.log(Infinity);
function divide(left, right) {
  return left / right;
}
console.log(divide(0, 0));
console.log(-NaN);
