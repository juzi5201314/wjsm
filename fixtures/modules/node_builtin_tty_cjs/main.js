const tty = require('tty');
console.log(tty === require('node:tty'));
console.log(tty.isatty(-1), tty.isatty(0.5), tty.isatty('2'), tty.isatty(4294967296));
console.log(typeof tty.isatty(0), typeof tty.isatty(1), typeof tty.isatty(2));
console.log(typeof tty.ReadStream, typeof tty.WriteStream);
console.log(typeof tty.ReadStream.prototype.setRawMode, typeof tty.WriteStream.prototype.getColorDepth);
