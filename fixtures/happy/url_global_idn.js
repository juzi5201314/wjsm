import { URL as ModURL } from 'node:url';

console.log(typeof globalThis.URL, typeof globalThis.URLSearchParams);
console.log(globalThis.URL === ModURL);

const u = new globalThis.URL('https://例え.テスト/x');
console.log(u.hostname, u.href);
const v6 = new globalThis.URL('http://[::1]:8080/x');
console.log(v6.hostname, v6.host, v6.href);
