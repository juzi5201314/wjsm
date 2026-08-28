import timersPromises from 'node:timers/promises';

function assertCallback(callback) {
  if (typeof callback !== 'function') {
    throw new TypeError('The "callback" argument must be of type function');
  }
}

function timersSetTimeout(callback, delay, ...args) {
  assertCallback(callback);
  if (args.length === 0) return setTimeout(callback, delay);
  if (args.length === 1) return setTimeout(callback, delay, args[0]);
  if (args.length === 2) return setTimeout(callback, delay, args[0], args[1]);
  if (args.length === 3) return setTimeout(callback, delay, args[0], args[1], args[2]);
  return setTimeout(callback, delay, args[0], args[1], args[2], args[3]);
}

function timersClearTimeout(handle) {
  clearTimeout(handle);
}

function timersSetInterval(callback, delay, ...args) {
  assertCallback(callback);
  if (args.length === 0) return setInterval(callback, delay);
  if (args.length === 1) return setInterval(callback, delay, args[0]);
  if (args.length === 2) return setInterval(callback, delay, args[0], args[1]);
  if (args.length === 3) return setInterval(callback, delay, args[0], args[1], args[2]);
  return setInterval(callback, delay, args[0], args[1], args[2], args[3]);
}

function timersClearInterval(handle) {
  clearInterval(handle);
}

const timersSetImmediate = globalThis.setImmediate;
const timersClearImmediate = globalThis.clearImmediate;
export const promises = timersPromises;

export {
  timersSetTimeout as setTimeout,
  timersClearTimeout as clearTimeout,
  timersSetInterval as setInterval,
  timersClearInterval as clearInterval,
  timersSetImmediate as setImmediate,
  timersClearImmediate as clearImmediate,
};
const timersDefault = {
  setTimeout: timersSetTimeout,
  clearTimeout: timersClearTimeout,
  setInterval: timersSetInterval,
  clearInterval: timersClearInterval,
  setImmediate: timersSetImmediate,
  clearImmediate: timersClearImmediate,
};
// 跨模块导入值不进对象字面量模板（builtin 段缓存的模板常量限制），改用属性赋值。
timersDefault.promises = timersPromises;
export default timersDefault;
