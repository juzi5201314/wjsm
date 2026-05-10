var obj = Promise.withResolvers();
console.log(typeof obj.promise);
console.log(typeof obj.resolve);
console.log(typeof obj.reject);
obj.resolve(42);
obj.promise.then(function(v) { console.log(v); });
