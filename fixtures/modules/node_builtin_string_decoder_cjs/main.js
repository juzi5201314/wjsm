const { StringDecoder } = require('string_decoder');
console.log(require('node:string_decoder').StringDecoder === StringDecoder);
const u16 = new StringDecoder('utf16le');
console.log(JSON.stringify(u16.write(Buffer.from([0x61]))));
console.log(JSON.stringify(u16.write(Buffer.from([0x00, 0x62, 0x00]))));
console.log(JSON.stringify(u16.end()));
const hex = new StringDecoder('hex');
console.log(hex.write(Buffer.from([0xde, 0xad])));
let threw = false;
try {
  new StringDecoder('nope');
} catch (e) {
  threw = e instanceof TypeError;
}
console.log(threw);
