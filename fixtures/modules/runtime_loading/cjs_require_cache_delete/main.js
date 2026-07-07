const depSpecifier = './dep' + '.js';
const first = require(depSpecifier);
const depId = require.resolve(depSpecifier);
console.log(first.count);
console.log(require(depSpecifier) === first);
console.log(delete require.cache[depId]);
const second = require(depSpecifier);
console.log(second.count);
console.log(second === first);
console.log(require.cache[depId].exports === second);

const jsonFirst = require('./data.json');
const jsonId = require.resolve('./data.json');
console.log(jsonFirst.marker);
console.log(delete require.cache[jsonId]);
const jsonSecond = require('./data.json');
console.log(jsonSecond.marker);
console.log(jsonSecond === jsonFirst);
console.log(require.cache[jsonId].exports === jsonSecond);
