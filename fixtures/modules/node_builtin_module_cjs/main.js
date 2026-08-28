const moduleExports = require('module');
console.log(moduleExports === require('node:module'));
console.log(typeof moduleExports, typeof moduleExports.createRequire, typeof moduleExports.isBuiltin);
console.log(
  Array.isArray(moduleExports.builtinModules),
  moduleExports.builtinModules.includes('module'),
  moduleExports.builtinModules.includes('tty'),
);
console.log(moduleExports.isBuiltin('tty'), moduleExports.isBuiltin('node:v8'), moduleExports.isBuiltin('nope'));

const requireFromHere = moduleExports.createRequire(__filename);
const dep = requireFromHere('./dep.js');
console.log(dep.kind, dep.triple(5));
console.log(requireFromHere('node:path').join('x', 'y'));
