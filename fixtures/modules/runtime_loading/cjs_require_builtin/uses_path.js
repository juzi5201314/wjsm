const pathA = require('node:path');
const pathB = require('node:path');
exports.joined = pathA.join('a', 'b');
exports.sameInstance = pathA === pathB;
