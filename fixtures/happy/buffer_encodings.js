console.log(Buffer.from('hello world').toString('hex'));
console.log(Buffer.from('hello world').toString('base64'));
console.log(Buffer.from('6869', 'hex').toString('utf8'));
console.log(Buffer.from('aGVsbG8=', 'base64').toString('utf8'));
console.log(Buffer.from('€', 'utf16le').toString('hex'));
console.log(Buffer.from([0xff]).toString('ascii').charCodeAt(0) + ':' + Buffer.from([0xff]).toString('latin1').charCodeAt(0));
