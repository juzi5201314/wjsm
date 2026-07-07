const part = './dep';
const dep = require(part + '.js');
console.log(dep.value);
console.log(require(part + '.js') === dep);
