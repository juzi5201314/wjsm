// Issue #156: toFixed/toExponential/toPrecision spec compliance
// toFixed: x >= 1e21 returns ToString(x)
console.log((1e21).toFixed(2));
console.log((-1e21).toFixed(2));
console.log((1.5e21).toFixed(5));

// toExponential: range check 0..=100
try { (123).toExponential(-1); } catch(e) { console.log("RangeError:", e.name); }
try { (123).toExponential(101); } catch(e) { console.log("RangeError:", e.name); }
console.log((123).toExponential(0));
console.log((123).toExponential(100));

// toPrecision: range 1..=100 (not 1..=21)
try { (123).toPrecision(0); } catch(e) { console.log("RangeError:", e.name); }
try { (123).toPrecision(101); } catch(e) { console.log("RangeError:", e.name); }
console.log((123).toPrecision(50));
console.log((123.456).toPrecision(1));
console.log((123.456).toPrecision(5));

// Infinity/NaN edge cases (per spec: return string, not throw)
console.log((Infinity).toFixed(2));
console.log((-Infinity).toFixed(2));
console.log((NaN).toFixed(2));
console.log((Infinity).toExponential(2));
console.log((Infinity).toPrecision(5));
