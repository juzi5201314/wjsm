const vm = require('vm');
const s = new vm.Script('1+1');
console.log(s.runInThisContext());
console.log(s.runInThisContext());
