import { format } from 'node:util';

const consoleObject = globalThis.console;

function isWritableLike(stream) {
  return stream !== null && typeof stream === 'object' && typeof stream.write === 'function';
}

// Node：Console 方法把全部实参（含 undefined）交给 util.format，不截断。
function formatArgs(args) {
  return format(...args);
}

export function Console(stdoutOrOptions, stderrMaybe) {
  let stdout = stdoutOrOptions;
  let stderr = stderrMaybe;
  if (stdoutOrOptions !== null && typeof stdoutOrOptions === 'object' && !isWritableLike(stdoutOrOptions)) {
    stdout = stdoutOrOptions.stdout;
    stderr = stdoutOrOptions.stderr;
  }
  if (!isWritableLike(stdout)) {
    throw new TypeError('Console expects a writable stream instance');
  }
  // Node：未提供 stderr 时警告/错误输出与 stdout 共用同一流。
  this._stdout = stdout;
  this._stderr = isWritableLike(stderr) ? stderr : stdout;
}

Console.prototype.log = function (...args) {
  this._stdout.write(formatArgs(args) + '\n');
};

Console.prototype.info = function (...args) {
  this._stdout.write(formatArgs(args) + '\n');
};

Console.prototype.debug = function (...args) {
  this._stdout.write(formatArgs(args) + '\n');
};

Console.prototype.error = function (...args) {
  this._stderr.write(formatArgs(args) + '\n');
};

Console.prototype.warn = function (...args) {
  this._stderr.write(formatArgs(args) + '\n');
};

Console.prototype.trace = function (...args) {
  const message = formatArgs(args);
  this._stderr.write('Trace' + (message ? ': ' + message : '') + '\n');
};

export const log = consoleObject.log;
export const info = consoleObject.info;
export const debug = consoleObject.debug;
export const warn = consoleObject.warn;
export const error = consoleObject.error;
export const trace = consoleObject.trace;

consoleObject.Console = Console;
export default consoleObject;
