const consoleModule = require('console');
console.log(consoleModule === require('node:console'));
console.log(consoleModule === console);
console.log(typeof consoleModule.Console === 'function');
const custom = new consoleModule.Console(process.stdout);
custom.log('cjs custom console');
