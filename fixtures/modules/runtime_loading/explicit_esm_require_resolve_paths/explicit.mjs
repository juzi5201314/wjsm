var require;
if (false) {
    require.resolve.paths('pkg');
}
console.log('mjs cjs bindings:', typeof module, typeof exports);
