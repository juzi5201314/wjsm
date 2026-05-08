// CJS circular dependency test
const a = require('./a.js');
console.log(a.value);
