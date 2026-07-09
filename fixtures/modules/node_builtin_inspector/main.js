import inspector, { open, close, url } from 'node:inspector';
console.log(typeof open, typeof close, typeof url);
console.log(typeof inspector.open, typeof inspector.url);
console.log(url() === undefined);
open();
// 无 CLI --inspect 时仍为 undefined
console.log(url() === undefined);
close();
console.log('ok');
