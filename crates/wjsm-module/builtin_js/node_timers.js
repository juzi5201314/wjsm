import timersPromises from 'node:timers/promises';

function assertCallback(callback) {
  if (typeof callback !== 'function') {
    throw new TypeError('The "callback" argument must be of type function');
  }
}

// 裸 setTimeout 标识符只在直接调用形式下被识别为 builtin，spread 调用需经
// globalThis 属性路径；宿主层完整转发全部额外参数（含显式 undefined）。
function timersSetTimeout(callback, delay, ...args) {
  assertCallback(callback);
  return globalThis.setTimeout(callback, delay, ...args);
}

function timersClearTimeout(handle) {
  clearTimeout(handle);
}

function timersSetInterval(callback, delay, ...args) {
  assertCallback(callback);
  return globalThis.setInterval(callback, delay, ...args);
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
