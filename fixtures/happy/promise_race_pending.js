let resolveA;
let resolveB;
let p1 = new Promise(resolve => { resolveA = resolve; });
let p2 = new Promise(resolve => { resolveB = resolve; });

Promise.race([p1, p2]).then(value => console.log(value));

queueMicrotask(() => resolveB(2));
queueMicrotask(() => resolveA(1));
