const posix = require('path/posix');
console.log(posix === require('node:path/posix'));
console.log(posix === require('path').posix);
console.log(posix.join('x', 'y'));
console.log(posix.normalize('a//b/../c/'));
