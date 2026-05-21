// Verify that eval can read/write bindings through the scope record
var x = 10;
var result = eval('x = x + 1; x');
console.log(result);
console.log(x);
