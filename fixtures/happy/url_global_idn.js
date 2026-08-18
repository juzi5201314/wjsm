import { URL as ModURL } from 'node:url';

console.log(typeof globalThis.URL, typeof globalThis.URLSearchParams);
console.log(globalThis.URL === ModURL);

const u = new globalThis.URL('https://例え.テスト/x');
console.log(u.hostname, u.href);
