// Test: fetch returns a Response with a ReadableStream body
const resp = await fetch("data:text/plain,hello");
console.log(resp.ok);           // true
console.log(typeof resp.body);  // object (ReadableStream)

// Get a reader
const reader = resp.body.getReader();
console.log(resp.body.locked);  // true

// Read chunk
const r1 = await reader.read();
console.log(r1.done);           // false
console.log(r1.value.length);   // 5 (hello)
