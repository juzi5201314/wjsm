// Test: WritableStream locked state
const stream = new WritableStream({
  write(chunk) {},
  close() {}
});
console.log(stream.locked);          // false
const writer = stream.getWriter();
console.log(stream.locked);          // true
writer.close().then(() => {
  console.log(stream.locked);       // false
});
