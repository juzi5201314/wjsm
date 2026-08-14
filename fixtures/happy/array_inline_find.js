let a = [0, "", null, 2, 3];
console.log("find", a.find(x => x));
console.log("findIndex", a.findIndex(x => x));
console.log("some", a.some(x => x));
console.log("every", a.every(x => x !== undefined));
console.log("every-false", a.every(x => x));
let n = [0, 1, 2];
console.log("find-num", n.find(x => x === 0));
