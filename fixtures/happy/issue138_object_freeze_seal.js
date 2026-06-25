const obj = { x: 1 };
Object.freeze(obj);
obj.x = 2;
console.log("frozen x:", obj.x);
console.log("isFrozen:", Object.isFrozen(obj));

const sealed = { a: 1, b: 2 };
Object.seal(sealed);
sealed.c = 3;
console.log("sealed c:", sealed.c);
console.log("isSealed:", Object.isSealed(sealed));

console.log("isFrozen empty:", Object.isFrozen({}));