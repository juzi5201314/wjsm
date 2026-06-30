// Error.stack property (issue #148)
const e = new Error("test");
console.log(typeof e.stack);
console.log(e.stack !== undefined);
console.log(e.stack.includes("Error"));

// TypeError stack
const te = new TypeError("type error");
console.log(typeof te.stack);
console.log(te.stack.includes("TypeError"));

// Error() without new
const e2 = Error("no new");
console.log(typeof e2.stack);

// Stack starts with error name and message
const e3 = new RangeError("range issue");
console.log(e3.stack.startsWith("RangeError: range issue"));
