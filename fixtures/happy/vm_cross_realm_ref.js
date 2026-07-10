const vm = require('vm');
const o = vm.runInNewContext('({a:2})');
console.log(o.a);
