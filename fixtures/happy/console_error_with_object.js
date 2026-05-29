// error with object argument — should serialize via render_value.
const obj = { type: "error-info", code: 500 };
console.error("Error occurred:", obj);
console.log("after-error-log");
