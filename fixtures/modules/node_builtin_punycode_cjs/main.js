const punycode = require('punycode');
console.log(punycode === require('node:punycode'));
console.log(punycode.toASCII('bücher.de'));
console.log(punycode.toUnicode('xn--bcher-kva.de'));
console.log(punycode.encode('münchen'));
console.log(punycode.decode('mnchen-3ya'));
