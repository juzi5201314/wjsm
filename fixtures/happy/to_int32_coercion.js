console.log('5' | 0);      // 5
console.log('true' | 0);    // 0 (NaN → 0)
console.log(true | 0);      // 1
console.log(false | 0);     // 0
console.log(null | 0);      // 0
console.log(undefined | 0); // 0
console.log(3.7 | 0);       // 3
console.log(-3.7 | 0);      // -3
console.log('10' & '7');    // 2
console.log(true << 2);     // 4
console.log(~'5');          // -6
console.log('8' << '2');    // 32