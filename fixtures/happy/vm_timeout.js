const vm = require('vm');
var timedOut = false;
try {
  vm.runInNewContext('while(true){}', {}, { timeout: 80 });
} catch (e) {
  var s = String(e && e.message || e);
  timedOut = s.indexOf('timed out') >= 0;
}
console.log(timedOut);
// 超时后主 realm 仍可用
console.log(1 + 1);
