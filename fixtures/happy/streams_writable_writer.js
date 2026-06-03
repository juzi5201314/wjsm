// Test: WritableStream writer methods
const stream = new WritableStream({
  write(chunk) {
    console.log("w:" + chunk);
  },
  close() {
    console.log("c");
  }
});
console.log(stream.locked);          // false
const writer = stream.getWriter();
console.log(stream.locked);          // true
writer.write("hello").then(() => {
  console.log("written");
  return writer.write("world");
}).then(() => {
  console.log("written2");
  return writer.close();
}).then(() => {
  console.log("writer-closed");
});
