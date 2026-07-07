const aSpecifier = './a' + '.js';
const bSpecifier = './b' + '.js';
const a = require(aSpecifier);
const b = require(bSpecifier);
console.log(a.fromA);
console.log(a.bSawA);
console.log(a.after);
console.log(b.sawAAfter === undefined);
console.log(require(aSpecifier) === a);
