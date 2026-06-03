// Test: cancel stream
const resp = await fetch("data:text/plain,data");
console.log(resp.body.locked);  // false
const result = await resp.body.cancel("no longer needed");
console.log(result);            // undefined
