// Abstract Equality Comparison (==) 测试
console.log(null == undefined);        // true
console.log(undefined == null);        // true
console.log(null == 0);                // false
console.log("2" == 2);                 // true
console.log(2 == "2");                 // true
console.log(true == 1);               // true
console.log(false == 0);              // true
console.log(true == 2);               // false
console.log(1 == true);               // true
console.log("" == false);             // true
console.log("" == 0);                 // true
console.log(0 == "");                 // true
console.log("5" == 5);                // true
console.log("5" != 5);                // false
console.log(NaN == NaN);              // false (NaN != NaN even in AbstractEq)
console.log(0 == -0);                 // true
console.log(1 == 1);                  // true (same type, strict path)
console.log("hello" == "hello");      // true (same type, strict path)
console.log(null == false);           // false
console.log(undefined == false);      // false
