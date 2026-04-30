let obj = { a: 1, b: 10, c: 5, d: 1, e: 1 };
// Arithmetic compound
obj.a += 2;
console.log(obj.a);
obj.b -= 3;
console.log(obj.b);
obj.c *= 2;
console.log(obj.c);
// Logical compound
let flag = { x: true, y: false, z: null };
flag.x &&= false;
console.log(flag.x);
flag.y ||= true;
console.log(flag.y);
flag.z ??= 42;
console.log(flag.z);
