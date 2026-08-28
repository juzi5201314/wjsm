import moduleDefault, { createRequire, isBuiltin, builtinModules } from 'node:module';

console.log(typeof moduleDefault, typeof createRequire, typeof isBuiltin);
console.log(
  Array.isArray(builtinModules),
  builtinModules.includes('path'),
  builtinModules.includes('stream/web'),
  builtinModules.includes('missing'),
);
console.log(isBuiltin('fs'), isBuiltin('node:fs'), isBuiltin('node:missing'), isBuiltin('missing'), isBuiltin(7));

const require = createRequire(import.meta.url);
const dep = require('./dep.js');
console.log(dep.flavor, dep.double(21));
const path = require('node:path');
console.log(path.join('a', 'b'), require('path') === path);

try {
  createRequire('relative/entry.js');
} catch (error) {
  console.log(error instanceof TypeError, error.code);
}
try {
  createRequire(42);
} catch (error) {
  console.log(error instanceof TypeError, error.code);
}

console.log(moduleDefault.createRequire === createRequire, moduleDefault.isBuiltin === isBuiltin);
