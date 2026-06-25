const o = { x: 1 };
console.log("own x:", Object.hasOwn(o, "x"));
console.log("own y:", Object.hasOwn(o, "y"));
const child = Object.create({ inherited: true });
child.own = 2;
console.log("own inherited:", Object.hasOwn(child, "inherited"));
console.log("own own:", Object.hasOwn(child, "own"));