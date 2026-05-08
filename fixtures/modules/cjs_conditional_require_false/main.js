// CJS conditional require with false branch
let lib;
if (false) {
    lib = require('./lib.js');
}
console.log(lib);
