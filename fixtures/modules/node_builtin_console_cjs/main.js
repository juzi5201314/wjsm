const consoleModule = require('console');
console.log(consoleModule === require('node:console'));
console.log(consoleModule === console);
console.log(typeof consoleModule.Console === 'function');
const custom = new consoleModule.Console(process.stdout);
custom.log('cjs custom console');
custom.log('cjs %s', undefined, 7, 8, 9, 10, 11, 12);
