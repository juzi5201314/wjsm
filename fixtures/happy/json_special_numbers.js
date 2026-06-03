// Special numeric values — per spec: NaN/Infinity/-Infinity all become null.
console.log(JSON.stringify(NaN));
console.log(JSON.stringify(Infinity));
console.log(JSON.stringify(-Infinity));
console.log(JSON.stringify({ x: NaN, y: Infinity }));
