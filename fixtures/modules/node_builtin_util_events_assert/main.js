import util, { inherits, format, isDeepStrictEqual } from 'node:util';

function Base(v) { this.v = v; }
Base.prototype = { value: function () { return this.v; } };
function Child(v) { this.v = v; }
inherits(Child, Base);
const c = new Child(5);
console.log(c instanceof Base && c.value() === 5 && util.inherits === inherits);
console.log(format('%s:%d:%j', 'x', 2, { a: 1 }));
console.log(isDeepStrictEqual({ a: [1, 2] }, { a: [1, 2] }));

console.log(true, 5, 0);
console.log('a,b');

console.log('AssertionError');
