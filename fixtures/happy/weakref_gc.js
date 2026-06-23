var obj = { tag: "alive" };
var ref = new WeakRef(obj);
console.log(ref.deref() !== undefined);
obj = null;
gc();
console.log(ref.deref() === undefined);