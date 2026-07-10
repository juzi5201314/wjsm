const vm = require('vm');
const a = vm.runInNewContext('[]');
console.log(a instanceof Array);
