let a = [1, 2, 3, 4, 5];
console.log(a.some(function(x) { return x > 3; }));
console.log(a.every(function(x) { return x > 3; }));
console.log(a.some(function(x) { return x > 10; }));
console.log(a.every(function(x) { return x < 10; }));
