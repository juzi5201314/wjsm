import crypto, { createHash, createHmac, randomBytes, randomUUID, getHashes } from 'node:crypto';
console.log(typeof crypto.createHash, typeof createHash, typeof createHmac);
console.log(createHash('sha1').update('abc').digest('hex'));
console.log(Buffer.isBuffer(randomBytes(2)));
console.log(randomUUID().length === 36);
console.log(getHashes().join(',') === 'md5,sha1,sha256,sha512');
