exports.fromA = 'a-start';
const b = require('./b' + '.js');
exports.bSawA = b.sawA;
exports.after = 'a-after';
