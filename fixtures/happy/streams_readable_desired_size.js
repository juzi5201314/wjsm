// Test: reader.closed promise
const resp = await fetch("data:text/plain,test");
const reader = resp.body.getReader();
console.log(typeof reader.closed);  // object (it's a Promise)

// Read chunk
const r1 = await reader.read();
console.log(r1.done);  // false
