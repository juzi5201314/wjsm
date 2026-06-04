// Test: Response.body.getReader() marks bodyUsed and reads data URL stream bytes
const resp = await fetch("data:text/plain,Hello");
console.log("bodyUsed before:", resp.bodyUsed);
const reader = resp.body.getReader();
console.log("bodyUsed after getReader:", resp.bodyUsed);
const first = await reader.read();
console.log("first done:", first.done);
console.log("first length:", first.value.length);
console.log("first byte:", first.value[0]);
reader.read().then(second => {
  console.log("second done:", second.done);
});
