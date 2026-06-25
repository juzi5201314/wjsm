// ECMAScript §7.1.4 ToNumber via Math (issue #126)
console.log(Math.abs("5"));
console.log(Math.abs({ valueOf() { return -3; } }));
try {
  Math.abs(Symbol("s"));
} catch (e) {
  console.log(e instanceof TypeError);
}