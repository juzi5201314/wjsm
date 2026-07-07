const paths = require.resolve.paths('pkg');
console.log(Array.isArray(paths));
console.log(paths[0].includes('modules/runtime_loading/require_resolve_paths/sub/node_modules'));
console.log(paths[1].includes('modules/runtime_loading/require_resolve_paths/node_modules'));
console.log(require.resolve.paths('./dep.js') === null);
console.log(require.resolve.paths('node:fs') === null);
