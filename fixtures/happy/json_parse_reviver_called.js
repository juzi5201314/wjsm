let hits = 0;
const result = JSON.parse('{"a":1}', function (key, value) {
  hits++;
  return value;
});
console.log("parse-hits:", hits);
console.log("a:", result.a);
