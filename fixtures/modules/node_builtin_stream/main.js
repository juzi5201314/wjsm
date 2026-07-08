const stream = require('stream');
const nodeStream = require('node:stream');
console.log(stream === nodeStream);

const pass = new stream.PassThrough();
pass.on('data', function (chunk) { console.log('data', Buffer.from(chunk).toString()); });
pass.on('end', function () { console.log('ended'); });
pass.write('hello');
pass.end(' world');

console.log(typeof stream.pipeline, typeof stream.finished);
