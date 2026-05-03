// Abstract Relational Comparison (<, >, <=, >=) 测试
console.log("a" < "b");           // true (string lexicographic)
console.log("apple" < "banana");  // true
console.log("z" < "a");           // false
console.log("2" < 10);            // true (string "2" → number 2 → 2 < 10)
console.log("10" < 2);            // false
console.log(null < 0);            // false (ToNumber(null)=+0, 0 < 0 → false)
console.log(null > 0);            // false (0 > 0 → false)
console.log(undefined < 0);       // false (ToNumber(undefined)=NaN → false)
console.log(5 < 10);              // true
console.log(5 > 10);              // false
console.log(5 <= 5);              // true
console.log(5 >= 5);              // true
console.log(5 <= 4);              // false
console.log(5 >= 6);              // false
console.log(true < 2);            // true (ToNumber(true)=1, 1<2)
console.log(false < 1);           // true (ToNumber(false)=0, 0<1)
console.log(3 > true);            // true
