const vm = require('vm');
const n = vm.runInThisContext('1+2');
console.log(n);
