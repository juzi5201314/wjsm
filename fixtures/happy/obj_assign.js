var target = {a: 1};
Object.assign(target, {b: 2}, {c: 3});
console.log(target.a);
console.log(target.b);
console.log(target.c);
