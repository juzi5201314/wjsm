// Special numeric values become null per spec.
console.log(JSON.stringify(NaN));
console.log(JSON.stringify(Infinity));
console.log(JSON.stringify(-Infinity));
console.log(JSON.stringify({ x: NaN, y: Infinity }));
