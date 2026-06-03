// Test: data: URL body should be a ReadableStream
const resp = await fetch("data:text/plain,Hello");
console.log(resp.ok);        // true
console.log(typeof resp.body);  // object (it's a ReadableStream, not null)

// Read the body via getReader
const reader = resp.body.getReader();
const r1 = await reader.read();
console.log(r1.done);        // false
// r1.value is a Uint8Array containing "Hello"
console.log(r1.value.length);  // 5
console.log(r1.value[0]);      // 72 (H)

// Also verify that text() still works on a fresh fetch
const resp2 = await fetch("data:text/plain,World");
const text = await resp2.text();
console.log(text);             // World
