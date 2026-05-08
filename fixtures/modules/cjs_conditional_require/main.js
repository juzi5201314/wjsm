// CJS conditional require test
let lib;
if (true) {
    lib = require('./lib.js');
}
console.log(lib.value);
