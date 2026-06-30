// Issue #151: Number() constructor handles BigInt, Symbol, objects
console.log(Number(1n));
console.log(Number(0n));
console.log(Number(-42n));
console.log(Number(123456789012345678901234567890n));

try { Number(Symbol()); } catch(e) { console.log("TypeError:", e.name); }
try { Number(Symbol("desc")); } catch(e) { console.log("TypeError:", e.name); }

console.log(Number({valueOf() { return 5; }}));
console.log(Number({valueOf() { return 42; }}));
console.log(Number({toString() { return "99"; }}));
console.log(Number({[Symbol.toPrimitive](hint) { return 7; }}));
