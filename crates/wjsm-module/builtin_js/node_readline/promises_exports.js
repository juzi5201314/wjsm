// node:readline/promises 导出段（与 core.js concat 成完整模块）。

class PromisesInterface extends Interface {
  // question 返回 Promise，由下一行 resolve；options.signal 未实现时显式报错，
  // 不做静默忽略的假实现。
  question(query, options) {
    if (options && options.signal) {
      return Promise.reject(new Error('readline question signal option is not implemented'));
    }
    if (this.closed) return Promise.reject(new Error('readline was closed'));
    const self = this;
    return new Promise(function (resolve) {
      writeToOutput(self, query);
      self._questionCallbacks.push(resolve);
    });
  }
}

// 光标操作的批量队列：commit 一次写出（autoCommit 时逐操作经 nextTick 写出）。
class Readline {
  constructor(stream, options) {
    if (!stream || typeof stream.write !== 'function') {
      throw new TypeError('The "stream" argument must be an instance of Writable');
    }
    this._stream = stream;
    this._todo = [];
    this._autoCommit = Boolean(options && options.autoCommit);
  }

  _push(data) {
    if (this._autoCommit) {
      const stream = this._stream;
      process.nextTick(function () {
        stream.write(data);
      });
    } else {
      this._todo.push(data);
    }
    return this;
  }

  cursorTo(x, y) {
    const data = typeof y !== 'number' ? kCSI + (x + 1) + 'G' : kCSI + (y + 1) + ';' + (x + 1) + 'H';
    return this._push(data);
  }

  moveCursor(dx, dy) {
    if (dx || dy) return this._push(moveCursorSequence(dx, dy));
    return this;
  }

  clearLine(dir) {
    return this._push(clearLineSequence(dir));
  }

  clearScreenDown() {
    return this._push(kCSI + '0J');
  }

  commit() {
    const self = this;
    return new Promise(function (resolve) {
      self._stream.write(self._todo.join(''), resolve);
      self._todo = [];
    });
  }

  rollback() {
    this._todo = [];
    return this;
  }
}

export { PromisesInterface as Interface, Readline };

export function createInterface(input, output, completer, terminal) {
  return new PromisesInterface(input, output, completer, terminal);
}

export default { Interface: PromisesInterface, Readline, createInterface };
