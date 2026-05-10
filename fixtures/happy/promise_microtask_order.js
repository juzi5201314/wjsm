console.log(1);
Promise.resolve().then(() => console.log(3));
queueMicrotask(() => console.log(2));
console.log(0);
