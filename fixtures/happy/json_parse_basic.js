// JSON.parse — KNOWN-BROKEN / STUB BEHAVIOR
// Current implementation is a stub: returns input string unchanged. Fixture documents intended behavior per spec.
const result = JSON.parse('{"a":1,"b":[2,3]}');
console.log("parse-result:", result);
console.log("type:", typeof result);