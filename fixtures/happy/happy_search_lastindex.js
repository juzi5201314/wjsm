// #199: String.prototype.search preserves lastIndex for global/sticky
var re = /test/g;
re.lastIndex = 5;
var result = "test test".search(re);
console.log(result === 0);            // search should find from start
console.log(re.lastIndex === 5);     // lastIndex restored after search

var re2 = /nomatch/g;
re2.lastIndex = 3;
var result2 = "no match".search(re2);
console.log(result2 === -1);
console.log(re2.lastIndex === 3);
