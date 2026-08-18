import { Buffer as Buf, transcode } from 'node:buffer';

console.log(Buf === globalThis.Buffer);
console.log(typeof transcode, typeof Buf.transcode);

const euro = transcode(Buffer.from('€'), 'utf8', 'ascii');
console.log(Buffer.isBuffer(euro), euro.toString('ascii'), euro[0]);

const cafe = transcode(Buffer.from('café', 'utf8'), 'utf8', 'ascii');
console.log(cafe.toString('ascii'));

const latin = transcode(new Uint8Array([0xff]), 'latin1', 'utf8');
console.log([...latin].join(','));

const asciiBad = transcode(Buffer.from([0xff]), 'ascii', 'utf8');
console.log([...asciiBad].join(','));

const round = transcode(Buffer.from([0xc3, 0xa9]), 'utf8', 'utf16le');
console.log([...round].join(','));

let errEnc = false;
try {
  transcode(Buffer.from('hi'), 'utf8', 'hex');
} catch (e) {
  errEnc = e instanceof Error && String(e.message).indexOf('U_ILLEGAL_ARGUMENT_ERROR') >= 0;
}
console.log(errEnc);

let errType = false;
try {
  transcode('hi', 'utf8', 'ascii');
} catch (e) {
  errType = e instanceof TypeError;
}
console.log(errType);
