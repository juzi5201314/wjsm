import { Buffer as Buf, transcode } from 'node:buffer';

const euro = transcode(Buffer.from('€'), 'utf8', 'ascii');
console.log(euro.toString('ascii'));

const utf16 = transcode(Buffer.from('ab', 'utf8'), 'utf8', 'utf16le');
console.log(utf16.length, utf16[0], utf16[1], utf16[2], utf16[3]);

const back = transcode(utf16, 'utf16le', 'utf8');
console.log(back.toString('utf8'));

const latin1 = transcode(Buffer.from([0xe9]), 'latin1', 'utf8');
console.log(latin1.toString('utf8'));
