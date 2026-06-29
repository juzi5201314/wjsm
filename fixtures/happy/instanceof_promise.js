// #2: instanceof Promise must work
console.log(Promise.resolve(1) instanceof Promise);
console.log(new Promise(() => {}) instanceof Promise);
console.log((async () => {})() instanceof Promise);
console.log(({}) instanceof Promise);
console.log(([]) instanceof Promise);
console.log(null instanceof Promise);
console.log(undefined instanceof Promise);
console.log(42 instanceof Promise);
console.log("string" instanceof Promise);
// Subclass
class MyPromise extends Promise {}
console.log(new MyPromise(() => {}) instanceof Promise);
console.log(new MyPromise(() => {}) instanceof MyPromise);
