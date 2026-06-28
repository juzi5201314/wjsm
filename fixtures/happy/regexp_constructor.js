var r1 = new RegExp("foo", "g");
console.log(r1.source, r1.flags, typeof r1);
var r2 = RegExp("ba(r)");
console.log(r2.source, r2.flags);
var r3 = new RegExp(/x/i);
console.log(r3.source, r3.flags);
console.log(typeof r1, typeof r2);
