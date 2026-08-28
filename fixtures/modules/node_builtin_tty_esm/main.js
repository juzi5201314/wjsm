import ttyDefault, { isatty, ReadStream, WriteStream } from 'node:tty';

console.log(typeof isatty, isatty.name, isatty.length);
// 非法 fd 一律 false（不触发真实探测）；合法 fd 只断言返回布尔，保持确定性。
console.log(isatty(-1), isatty(1.5), isatty(NaN), isatty(2147483648), isatty('1'));
console.log(typeof isatty(0), typeof isatty(1), typeof isatty(2));
console.log(typeof ReadStream, ReadStream.name, typeof ReadStream.prototype.setRawMode);
console.log(typeof WriteStream, WriteStream.name);
const writeProto = WriteStream.prototype;
console.log(
  typeof writeProto.cursorTo,
  typeof writeProto.moveCursor,
  typeof writeProto.clearLine,
  typeof writeProto.clearScreenDown,
);
console.log(typeof writeProto.getColorDepth, typeof writeProto.hasColors, typeof writeProto.getWindowSize);
console.log(ttyDefault.isatty === isatty, ttyDefault.ReadStream === ReadStream, ttyDefault.WriteStream === WriteStream);
