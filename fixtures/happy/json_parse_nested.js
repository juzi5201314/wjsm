// JSON.parse nested array/object/null parse.
const result = JSON.parse('[1,{"x":true},null]');
console.log("nested-result:", result);