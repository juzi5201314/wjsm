const data = require('./data.json');
console.log(data.name);
console.log(data.count);
console.log(data.nested.ok);
console.log(require('./data.json') === data);
console.log(require.cache[require.resolve('./data.json')].exports === data);
