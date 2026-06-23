var a = 'hello';
var b = 'hel' + 'lo';
console.log(a === b);
console.log(a !== b);

// Dynamic strings
var s1 = String(123);
var s2 = String(123);
console.log(s1 === s2);

// Different strings
console.log('abc' === 'abd');

// String vs non-string
console.log('5' === 5);
console.log(5 === '5');