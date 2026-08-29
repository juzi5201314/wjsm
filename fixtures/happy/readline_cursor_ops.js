// readline 模块级光标函数与 readline/promises Readline 队列：转义序列、
// null 流回退、commit/rollback。输出与 Node v22 逐字节一致（oracle 校验）。
import readline from 'node:readline';
import { Readline } from 'node:readline/promises';
import { Writable } from 'node:stream';

const written = [];
const sink = { write(data) { written.push(data); return true; } };

console.log(
  readline.cursorTo(sink, 3),
  readline.cursorTo(sink, 3, 7),
  readline.moveCursor(sink, -2, 4),
  readline.moveCursor(sink, 0, 0),
  readline.clearLine(sink, -1),
  readline.clearLine(sink, 0),
  readline.clearLine(sink, 1),
  readline.clearScreenDown(sink),
  readline.cursorTo(null, 1),
);
console.log(JSON.stringify(written));

const committed = [];
const writable = new Writable();
writable._write = function (chunk, encoding, callback) {
  committed.push(chunk.toString());
  callback();
};
const cursor = new Readline(writable);
console.log(
  cursor.cursorTo(2, 5) === cursor,
  cursor.moveCursor(-1, 2) === cursor,
  cursor.clearLine(0) === cursor,
  cursor.clearScreenDown() === cursor,
);
cursor.rollback();
cursor.cursorTo(0);
await cursor.commit();
console.log(JSON.stringify(committed));
