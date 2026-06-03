// Test: WritableStream constructor
const stream = new WritableStream({
  start(controller) {
    console.log("started");
  },
  write(chunk, controller) {
    console.log("write:" + chunk);
  },
  close(controller) {
    console.log("closed");
  },
  abort(reason) {
    console.log("abort:" + reason);
  }
});
console.log(typeof stream);          // object
console.log(stream.locked);          // false
