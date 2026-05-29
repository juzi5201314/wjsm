// JSON.parse (nested) — KNOWN-BROKEN / STUB BEHAVIOR
// Current implementation is a stub: returns input string unchanged. Fixture documents intended behavior per spec.
const result = JSON.parse('[1,{"x":true},null]');
console.log("nested-result:", result);