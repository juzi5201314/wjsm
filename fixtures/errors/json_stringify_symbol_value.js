// Symbol as top-level JSON value — spec: returns undefined (no output).
const sym = Symbol("test");
const result = JSON.stringify(sym);
console.log("symbol-top-level:", result);
console.log("typeof-result:", typeof result);
