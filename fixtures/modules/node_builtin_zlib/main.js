const zlib = require('zlib');
const nodeZlib = require('node:zlib');
console.log(zlib === nodeZlib);

const input = Buffer.from('hello zlib');
const gz = zlib.gzipSync(input);
console.log(Buffer.isBuffer(gz), zlib.gunzipSync(gz).toString());
const deflated = zlib.deflateSync(input);
console.log(zlib.inflateSync(deflated).toString());
const raw = zlib.deflateRawSync(input);
console.log(zlib.inflateRawSync(raw).toString());
const br = zlib.brotliCompressSync(input);
console.log(zlib.brotliDecompressSync(br).toString());
