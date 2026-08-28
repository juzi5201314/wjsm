const types = require('util/types');
console.log(types === require('node:util/types'));
console.log(types === require('util').types);
console.log(types.isDate(new Date()), types.isDate('2020-01-01'));
console.log(types.isMap(new Map()), types.isSet(new Set()));
console.log(types.isPromise(Promise.resolve()), types.isPromise({ then() {} }));
console.log(types.isNativeError(new RangeError('r')));
