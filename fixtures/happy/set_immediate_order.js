const seq = [];
setTimeout(() => seq.push('T'), 0);
setImmediate(() => seq.push('I'));
process.nextTick(() => seq.push('N'));
setTimeout(() => console.log(seq.join(',')), 0);
