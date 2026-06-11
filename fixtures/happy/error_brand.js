// Test Error brand: real Error vs plain object with error-like shape.

const realError = new TypeError("real error");
console.log("real:", realError); // Should render as "TypeError: real error"

const fakeError = { name: "TypeError", message: "fake error" };
console.log("fake:", fakeError); // Should render as "[object Object]"

console.log("PASS");
