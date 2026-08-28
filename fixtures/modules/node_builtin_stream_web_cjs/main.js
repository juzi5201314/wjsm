const sw = require('stream/web');
console.log(sw === require('node:stream/web'));
console.log(typeof sw.ReadableStream, sw.ReadableStream.name);
console.log(typeof sw.WritableStream, typeof sw.TransformStream);
console.log(new sw.CountQueuingStrategy({ highWaterMark: 2 }).highWaterMark);
const source = new sw.ReadableStream({
  start(controller) {
    controller.enqueue('cjs-chunk');
    controller.close();
  },
});
source
  .getReader()
  .read()
  .then((result) => {
    console.log(result.value, result.done);
  });
