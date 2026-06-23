// TypedArray indexOf / lastIndexOf 负数 fromIndex
var arr = new Int8Array([10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
console.log("indexOf -5: " + arr.indexOf(60, -5));
console.log("lastIndexOf -5: " + arr.lastIndexOf(60, -5));

var arr2 = new Int8Array([10, 20, 30]);
console.log("indexOf -10: " + arr2.indexOf(20, -10));
console.log("indexOf past length: " + arr2.indexOf(20, 10));
console.log("indexOf default: " + arr2.indexOf(20));
console.log("lastIndexOf default: " + arr2.lastIndexOf(20));