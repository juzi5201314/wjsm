import swDefault, {
  ReadableStream,
  WritableStream,
  TransformStream,
  CountQueuingStrategy,
  ByteLengthQueuingStrategy,
} from 'node:stream/web';

console.log(typeof ReadableStream, ReadableStream.name, ReadableStream.length);
console.log(typeof WritableStream, WritableStream.name);
console.log(typeof TransformStream, TransformStream.name);
console.log(new CountQueuingStrategy({ highWaterMark: 4 }).highWaterMark);
console.log(new ByteLengthQueuingStrategy({ highWaterMark: 32 }).highWaterMark);
console.log(swDefault.ReadableStream === ReadableStream);

const upper = new TransformStream({
  transform(chunk, controller) {
    controller.enqueue(chunk.toUpperCase());
  },
});
const source = new ReadableStream({
  start(controller) {
    controller.enqueue('ab');
    controller.enqueue('cd');
    controller.close();
  },
});
const reader = source.pipeThrough(upper).getReader();
let next = await reader.read();
while (!next.done) {
  console.log(next.value);
  next = await reader.read();
}
console.log('done', next.done);

const sink = [];
const writable = new WritableStream({
  write(chunk) {
    sink.push(chunk);
  },
});
await new ReadableStream({
  start(controller) {
    controller.enqueue(1);
    controller.enqueue(2);
    controller.close();
  },
}).pipeTo(writable);
console.log(sink.join('+'));
