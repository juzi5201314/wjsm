const win32 = require('path/win32');
console.log(win32 === require('node:path/win32'));
console.log(win32 === require('path').win32);
console.log(win32.basename('C:\\a\\b.js'));
console.log(win32.join('a', 'b'));
