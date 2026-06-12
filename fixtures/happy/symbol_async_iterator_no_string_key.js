async function* gen() {}
const g = gen();
console.log(JSON.stringify(Object.keys(g)));
console.log(Object.getOwnPropertySymbols(g).length);
console.log(Object.getOwnPropertySymbols(g)[0] === Symbol.asyncIterator);

const asyncIteratorProto = Object.getPrototypeOf(Object.getPrototypeOf(g));
console.log(JSON.stringify(Object.getOwnPropertyNames(asyncIteratorProto)));
console.log(Object.getOwnPropertySymbols(asyncIteratorProto).length);

const stream = new ReadableStream({
  start(controller) {
    controller.close();
  }
});
console.log(JSON.stringify(Object.keys(stream)));
console.log(Object.getOwnPropertySymbols(stream).length);

const transform = new TransformStream();
console.log(JSON.stringify(Object.keys(transform.readable)));
console.log(Object.getOwnPropertySymbols(transform.readable).length);
