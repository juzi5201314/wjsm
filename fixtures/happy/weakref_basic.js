var obj = { x: 1 };
var ref = new WeakRef(obj);
console.log(ref.deref().x);
console.log(ref.deref() === obj);