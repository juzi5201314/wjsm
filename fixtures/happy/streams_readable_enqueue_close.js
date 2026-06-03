// Test: reading a chunk from a ReadableStream (via fetch body)
const resp = await fetch("data:text/plain,ABC");
const reader = resp.body.getReader();

// Read chunk (all 3 bytes as Uint8Array)
const r1 = await reader.read();
console.log(r1.done);         // false
console.log(r1.value.length); // 3
console.log(r1.value[0]);     // 65 (A)
console.log(r1.value[1]);     // 66 (B)
console.log(r1.value[2]);     // 67 (C)
