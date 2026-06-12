let n = 42;
let fixed = n.toFixed;
console.log(fixed.call(n, 2));
console.log(n["toFixed"](2));
console.log((3.14159).toExponential(2));
console.log((123.456).toPrecision(4));
