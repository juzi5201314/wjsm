// node:readline 共享核心（callback 与 promises 封装经 concat 共用本段）。
// 覆盖非终端（管道）路径：按 \r\n / \n / 孤立 \r 拆行、question 拦截、
// 异步迭代与光标转义序列写出。终端逐键编辑/历史/补全需要原始 TTY 流，
// 不在当前宿主能力范围（见 node-compatibility-matrix.md）。
import { EventEmitter } from 'events';
import { StringDecoder } from 'string_decoder';

const kCSI = '\u001b[';

function writeToOutput(rl, data) {
  if (rl.output === null || rl.output === undefined) return;
  rl.output.write(data);
}

function emitLine(rl, line) {
  if (rl._questionCallbacks.length > 0) {
    const cb = rl._questionCallbacks.shift();
    cb(line);
    return;
  }
  rl.emit('line', line);
}

// 按 Node 非终端 kNormalWrite 语义拆行：\r\n 与 \n 为行界，孤立 \r 亦为行界；
// 块尾的 \r 留待下一块（或 EOF）判定是否与 \n 合并（crlfDelay 的确定性近似）。
function normalWrite(rl, data) {
  if (rl.closed) return;
  let text = typeof data === 'string' ? data : rl._decoder.write(data);
  text = rl._lineBuffer + text;
  rl._lineBuffer = '';
  const lines = [];
  let start = 0;
  let i = 0;
  while (i < text.length) {
    const ch = text.charAt(i);
    if (ch === '\n') {
      lines.push(text.slice(start, i));
      i = i + 1;
      start = i;
    } else if (ch === '\r') {
      if (i + 1 >= text.length) break;
      lines.push(text.slice(start, i));
      i = text.charAt(i + 1) === '\n' ? i + 2 : i + 1;
      start = i;
    } else {
      i = i + 1;
    }
  }
  rl._lineBuffer = text.slice(start);
  for (let j = 0; j < lines.length; j = j + 1) emitLine(rl, lines[j]);
}

function inputEnded(rl) {
  if (rl.closed) return;
  if (rl._lineBuffer) {
    let rest = rl._lineBuffer;
    rl._lineBuffer = '';
    if (rest.charAt(rest.length - 1) === '\r') rest = rest.slice(0, rest.length - 1);
    emitLine(rl, rest);
  }
  rl.close();
}

class Interface extends EventEmitter {
  // Node 双形态签名：createInterface(options) 或 (input, output, completer, terminal)。
  constructor(inputOrOptions, output, completer, terminal) {
    super();
    let input = inputOrOptions;
    let prompt = '> ';
    if (inputOrOptions && inputOrOptions.input) {
      const options = inputOrOptions;
      input = options.input;
      output = options.output;
      completer = options.completer;
      terminal = options.terminal;
      if (options.prompt !== undefined) prompt = options.prompt;
    }
    this.input = input;
    this.output = output;
    this.completer = completer;
    // 与 Node 一致：未显式指定时由 output.isTTY 决定；当前实现恒走非终端路径。
    this.terminal = Boolean(terminal === undefined ? output && output.isTTY : terminal);
    this.line = '';
    this.cursor = 0;
    this.closed = false;
    this.paused = false;
    this._prompt = prompt;
    this._lineBuffer = '';
    this._decoder = new StringDecoder('utf8');
    this._questionCallbacks = [];
    const self = this;
    this._onData = function (data) {
      normalWrite(self, data);
    };
    this._onEnd = function () {
      inputEnded(self);
    };
    input.on('data', this._onData);
    input.on('end', this._onEnd);
    if (typeof input.resume === 'function') input.resume();
  }

  close() {
    if (this.closed) return;
    if (this.input && typeof this.input.removeListener === 'function') {
      this.input.removeListener('data', this._onData);
      this.input.removeListener('end', this._onEnd);
    }
    this.pause();
    this.closed = true;
    this.emit('close');
  }

  pause() {
    if (this.paused) return;
    if (this.input && typeof this.input.pause === 'function') this.input.pause();
    this.paused = true;
    this.emit('pause');
    return this;
  }

  resume() {
    if (!this.paused) return;
    if (this.input && typeof this.input.resume === 'function') this.input.resume();
    this.paused = false;
    this.emit('resume');
    return this;
  }

  setPrompt(prompt) {
    this._prompt = prompt;
  }

  getPrompt() {
    return this._prompt;
  }

  prompt() {
    if (this.paused) this.resume();
    writeToOutput(this, this._prompt);
  }

  // 下一行优先交给 question 回调（不再触发 'line'，与 Node 拦截语义一致）。
  question(query, options, cb) {
    if (typeof options === 'function') cb = options;
    else if (options && options.signal) {
      throw new Error('readline question signal option is not implemented');
    }
    if (this.closed) throw new Error('readline was closed');
    writeToOutput(this, query);
    if (typeof cb === 'function') this._questionCallbacks.push(cb);
  }

  write(data) {
    if (this.closed) return;
    if (this.paused) this.resume();
    if (data === undefined || data === null) return;
    normalWrite(this, data);
  }
}

Interface.prototype[Symbol.asyncIterator] = function () {
  const self = this;
  const pendingLines = [];
  const pendingResolvers = [];
  let finished = self.closed;
  function onLine(line) {
    if (pendingResolvers.length > 0) pendingResolvers.shift()({ value: line, done: false });
    else pendingLines.push(line);
  }
  function onClose() {
    finished = true;
    while (pendingResolvers.length > 0) {
      pendingResolvers.shift()({ value: undefined, done: true });
    }
  }
  if (!finished) {
    self.on('line', onLine);
    self.on('close', onClose);
  }
  const iterator = {
    next() {
      if (pendingLines.length > 0) {
        return Promise.resolve({ value: pendingLines.shift(), done: false });
      }
      if (finished) return Promise.resolve({ value: undefined, done: true });
      return new Promise(function (resolve) {
        pendingResolvers.push(resolve);
      });
    },
    // for-await break：关闭 interface 并结束迭代（Node 语义）。
    return() {
      self.close();
      onClose();
      return Promise.resolve({ value: undefined, done: true });
    },
  };
  iterator[Symbol.asyncIterator] = function () {
    return iterator;
  };
  return iterator;
};

// 模块级光标函数的转义序列与 Node internal/readline/utils 一致。
function streamCursorTo(stream, x, y, callback) {
  if (typeof y === 'function') {
    callback = y;
    y = undefined;
  }
  if (stream === null || stream === undefined || (typeof x !== 'number' && typeof y !== 'number')) {
    if (typeof callback === 'function') process.nextTick(callback, null);
    return true;
  }
  if (typeof x !== 'number') throw new TypeError('Cannot set cursor row without setting its column');
  const data = typeof y !== 'number' ? kCSI + (x + 1) + 'G' : kCSI + (y + 1) + ';' + (x + 1) + 'H';
  return stream.write(data, callback);
}

function moveCursorSequence(dx, dy) {
  let data = '';
  if (dx < 0) data = data + kCSI + -dx + 'D';
  else if (dx > 0) data = data + kCSI + dx + 'C';
  if (dy < 0) data = data + kCSI + -dy + 'A';
  else if (dy > 0) data = data + kCSI + dy + 'B';
  return data;
}

function streamMoveCursor(stream, dx, dy, callback) {
  if (stream === null || stream === undefined || !(dx || dy)) {
    if (typeof callback === 'function') process.nextTick(callback, null);
    return true;
  }
  return stream.write(moveCursorSequence(dx, dy), callback);
}

function clearLineSequence(dir) {
  if (dir < 0) return kCSI + '1K';
  if (dir > 0) return kCSI + '0K';
  return kCSI + '2K';
}

function streamClearLine(stream, dir, callback) {
  if (stream === null || stream === undefined) {
    if (typeof callback === 'function') process.nextTick(callback, null);
    return true;
  }
  return stream.write(clearLineSequence(dir), callback);
}

function streamClearScreenDown(stream, callback) {
  if (stream === null || stream === undefined) {
    if (typeof callback === 'function') process.nextTick(callback, null);
    return true;
  }
  return stream.write(kCSI + '0J', callback);
}
