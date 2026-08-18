function bytes() {
  const out = new Uint8Array(arguments.length);
  for (let i = 0; i < arguments.length; i = i + 1) out[i] = arguments[i];
  return out;
}

const utf8 = new TextDecoder();
console.log(utf8.encoding, utf8.fatal, utf8.ignoreBOM);

const win = new TextDecoder('windows-1252');
console.log(win.encoding, win.decode(bytes(0x80)));

const gbk = new TextDecoder('gbk');
console.log(gbk.encoding, gbk.decode(bytes(0xd6, 0xd0)));

const bom = bytes(0xEF, 0xBB, 0xBF, 0x61);
console.log(JSON.stringify(new TextDecoder().decode(bom)));
console.log(JSON.stringify(new TextDecoder('utf-8', { ignoreBOM: true }).decode(bom)));

let fatalOk = false;
try {
  new TextDecoder('utf-8', { fatal: true }).decode(bytes(0xFF));
} catch (e) {
  fatalOk = e instanceof TypeError;
}
console.log(fatalOk);

const stream = new TextDecoder();
console.log(JSON.stringify(stream.decode(bytes(0xC3), { stream: true })));
console.log(JSON.stringify(stream.decode(bytes(0xA9))));

const sjis = new TextDecoder('shift_jis');
console.log(JSON.stringify(sjis.decode(bytes(0x82), { stream: true })));
console.log(JSON.stringify(sjis.decode(bytes(0xA0))));

const desc = Object.getOwnPropertyDescriptor(TextDecoder.prototype, 'encoding');
console.log(typeof desc.get, desc.set === undefined, desc.enumerable, desc.configurable);

console.log(new TextDecoder().decode(Buffer.from([0x68, 0x69])));
