import { format } from 'node:util';

const consoleObject = globalThis.console;

function isWritableLike(stream) {
  return stream !== null && typeof stream === 'object' && typeof stream.write === 'function';
}

function formatArgs(args) {
  if (args.length === 0) return '';
  if (args.length === 1) return format(args[0]);
  if (args.length === 2) return format(args[0], args[1]);
  if (args.length === 3) return format(args[0], args[1], args[2]);
  if (args.length === 4) return format(args[0], args[1], args[2], args[3]);
  if (args.length === 5) return format(args[0], args[1], args[2], args[3], args[4]);
  return format(args[0], args[1], args[2], args[3], args[4], args[5]);
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
