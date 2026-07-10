const vm = require('vm');
var n = 0;
try { vm.measureMemory(); } catch (e) { if (String(e && e.message || e).indexOf('not implemented') >= 0) n++; }
try { new vm.SourceTextModule('export default 1'); } catch (e) { if (String(e && e.message || e).indexOf('not implemented') >= 0) n++; }
try { new vm.SyntheticModule([], function () {}); } catch (e) { if (String(e && e.message || e).indexOf('not implemented') >= 0) n++; }
console.log(n);
