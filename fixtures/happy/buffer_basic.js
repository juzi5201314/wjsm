console.log(Buffer.isBuffer(Buffer.alloc(1)), Buffer.alloc(4).length, Buffer.from('ABC').toString());
console.log(Buffer.from([257, 257.5, -255, '1']).toJSON().data.join(','));
console.log(Buffer.byteLength('tést', 'utf8'), Buffer.from('tést', 'latin1').toString('hex'));
console.log(Buffer.concat([Buffer.from('he'), Buffer.from('llo')]).toString());
