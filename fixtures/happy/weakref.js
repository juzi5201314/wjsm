let obj = { x: 1 };
let wr = new WeakRef(obj);
console.log(wr.deref().x); // should print 1

// Test with null target isn't valid (TypeError), test basic deref
let wr2 = new WeakRef({ a: 2 });
let result = wr2.deref();
console.log(result !== undefined); // true - object still alive

// Test unregistered wr
let obj3 = { b: 3 };
let wr3 = new WeakRef(obj3);
obj3 = null; // remove strong reference
console.log('typeof wr3.deref():', typeof wr3.deref()); // 'object' or 'undefined' depending on GC state
