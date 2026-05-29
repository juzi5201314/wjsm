// Special numeric values — documents actual behavior (spec gap for NaN).
// Per spec: NaN/Infinity/-Infinity all become null. Actual: NaN outputs undefined (spec gap), Infinity/-Infinity output null.
console.log(JSON.stringify(NaN));
console.log(JSON.stringify(Infinity));
console.log(JSON.stringify(-Infinity));
console.log(JSON.stringify({ x: NaN, y: Infinity }));
