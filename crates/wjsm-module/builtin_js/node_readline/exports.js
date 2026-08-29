// node:readline 导出段（与 core.js concat 成完整模块）。
// emitKeypressEvents 依赖终端逐键流，当前不提供（见兼容矩阵）。
// promises 用命名空间导入拼装（Node 的 readline.promises 即 CJS exports 对象）。
import * as promisesNs from 'readline/promises';

const promises = {
  Interface: promisesNs.Interface,
  Readline: promisesNs.Readline,
  createInterface: promisesNs.createInterface,
};

export { Interface, promises };

export function createInterface(input, output, completer, terminal) {
  return new Interface(input, output, completer, terminal);
}

export function cursorTo(stream, x, y, callback) {
  return streamCursorTo(stream, x, y, callback);
}

export function moveCursor(stream, dx, dy, callback) {
  return streamMoveCursor(stream, dx, dy, callback);
}

export function clearLine(stream, dir, callback) {
  return streamClearLine(stream, dir, callback);
}

export function clearScreenDown(stream, callback) {
  return streamClearScreenDown(stream, callback);
}

export default {
  Interface,
  promises,
  createInterface,
  cursorTo,
  moveCursor,
  clearLine,
  clearScreenDown,
};
