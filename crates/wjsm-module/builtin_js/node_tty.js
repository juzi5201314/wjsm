// node:tty — isatty 真实终端探测 + ReadStream/WriteStream 形状。
// isatty 经 __wjsm_node_tty host bridge 做真实 fd 查询；
// ReadStream/WriteStream 仅提供 Node 形状（wjsm 的 stdio 不挂接原始终端流）。

function getHost() {
  const host = globalThis.__wjsm_node_tty;
  if (!host) throw new Error('wjsm internal tty host bridge is not installed');
  return host;
}

const host = getHost();

export function isatty(fd) {
  if (typeof fd !== 'number' || !Number.isInteger(fd) || fd < 0 || fd > 2147483647) {
    return false;
  }
  return host.isatty(fd);
}

export function ReadStream(fd, options) {
  if (!(this instanceof ReadStream)) return new ReadStream(fd, options);
  this.fd = fd;
  // 真实探测而非固定 true：Node 在非 tty fd 上根本无法构造成功。
  this.isTTY = isatty(fd);
  this.isRaw = false;
  this.readable = true;
}

ReadStream.prototype.setRawMode = function (mode) {
  this.isRaw = Boolean(mode);
  return this;
};

export function WriteStream(fd) {
  if (!(this instanceof WriteStream)) return new WriteStream(fd);
  this.fd = fd;
  this.isTTY = isatty(fd);
  this.writable = true;
  this.columns = undefined;
  this.rows = undefined;
}

WriteStream.prototype.clearLine = function (dir, callback) {
  if (typeof callback === 'function') callback();
  return true;
};

WriteStream.prototype.clearScreenDown = function (callback) {
  if (typeof callback === 'function') callback();
  return true;
};

WriteStream.prototype.cursorTo = function (x, y, callback) {
  if (typeof y === 'function') {
    callback = y;
  }
  if (typeof callback === 'function') callback();
  return true;
};

WriteStream.prototype.moveCursor = function (dx, dy, callback) {
  if (typeof callback === 'function') callback();
  return true;
};

WriteStream.prototype.getWindowSize = function () {
  return [this.columns, this.rows];
};

// 无终端能力探测数据时按 Node 的兜底口径报告 1 位色深（不伪造更高支持）。
WriteStream.prototype.getColorDepth = function () {
  return 1;
};

WriteStream.prototype.hasColors = function (count) {
  if (count === undefined) count = 16;
  return count <= 2 ** this.getColorDepth();
};

export default { isatty, ReadStream, WriteStream };
