// Test: locked getter behavior
const resp = await fetch("data:text/plain,test");
console.log(resp.body.locked);  // false
const reader = resp.body.getReader();
console.log(resp.body.locked);  // true
reader.releaseLock();
console.log(resp.body.locked);  // false
