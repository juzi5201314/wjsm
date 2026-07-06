import { URL, URLSearchParams, parse, format, resolve, pathToFileURL, fileURLToPath, domainToASCII, domainToUnicode } from 'node:url';
import qs from 'querystring';

const u = new URL('/a?x=1#h', 'https://example.com/base');
u.searchParams.append('y', '2');
console.log(u.href);

const params = new URLSearchParams('a=1&a=2');
console.log(1, params.has('a') ? 1 : 0);

const parsed = parse('https://example.com:8080/p?q=1#h');
console.log(parsed.protocol, parsed.hostname, parsed.port, parsed.pathname, parsed.search, parsed.hash);

console.log(format({ protocol: 'https:', hostname: 'example.com', pathname: '/x', query: { a: 1 } }));
console.log(resolve('https://e/a/b', '../c'));
console.log(fileURLToPath(pathToFileURL('/tmp/a b')));
console.log(domainToASCII('mañana.com'), domainToUnicode('xn--maana-pta.com'));
console.log(qs.stringify({ a: [1, 2], b: 'x y' }));
