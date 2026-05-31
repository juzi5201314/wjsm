const gap = String.fromCodePoint(0x1f600).repeat(6);
const obj = { a: 1 };
console.log(JSON.stringify(obj, null, gap));
