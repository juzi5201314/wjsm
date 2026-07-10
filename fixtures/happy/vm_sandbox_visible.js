const vm = require('vm');
const s = {};
vm.runInNewContext('x=1', s);
console.log(s.x);
