// util/types：只提供在本运行时可精确判定的品牌检查。
// typed array / ArrayBuffer 构造器在宿主里不支持 instanceof，对应 API 不提供，
// 避免返回错误答案（缺失优于错误）。

export function isDate(value) {
  return value instanceof Date;
}

export function isMap(value) {
  return value instanceof Map;
}

export function isSet(value) {
  return value instanceof Set;
}

export function isWeakMap(value) {
  return value instanceof WeakMap;
}

export function isWeakSet(value) {
  return value instanceof WeakSet;
}

export function isRegExp(value) {
  return value instanceof RegExp;
}

export function isPromise(value) {
  return value instanceof Promise;
}

export function isNativeError(value) {
  return value instanceof Error;
}

export function isProxy(value) {
  return false;
}

const typesObject = {
  isDate,
  isMap,
  isSet,
  isWeakMap,
  isWeakSet,
  isRegExp,
  isPromise,
  isNativeError,
  isProxy,
};
export default typesObject;
