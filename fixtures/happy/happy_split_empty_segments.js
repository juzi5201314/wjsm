// #198: String.prototype.split preserves empty segments and trailing empty
var r1 = "a,b,".split(/,/);
console.log(r1.length === 3);
console.log(JSON.stringify(r1[0]));
console.log(JSON.stringify(r1[1]));
console.log(JSON.stringify(r1[2]));

var r2 = ",a".split(/,/);
console.log(r2.length === 2);
console.log(JSON.stringify(r2[0]));
console.log(JSON.stringify(r2[1]));
