var C = BigInt64Array;
var x = new C(2);
console.log(x.length);
x[0] = -1n;
console.log(x[0]);

var D = BigUint64Array;
var y = new D(1);
y[0] = -1n;
console.log(y[0]);
