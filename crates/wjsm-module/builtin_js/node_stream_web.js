// node:stream/web — re-export 既有 Web Streams 构造器。
// 构造器值来自 __wjsm_web_streams host bridge，与全局调用点拦截命中的是同一实现，
// 因此 `new ReadableStream(...)`（全局）与本模块导出构造的是同一种流对象。

function getHost() {
  const host = globalThis.__wjsm_web_streams;
  if (!host) throw new Error('wjsm internal web streams host bridge is not installed');
  return host;
}

const host = getHost();

export const ReadableStream = host.ReadableStream;
export const WritableStream = host.WritableStream;
export const TransformStream = host.TransformStream;
export const CountQueuingStrategy = host.CountQueuingStrategy;
export const ByteLengthQueuingStrategy = host.ByteLengthQueuingStrategy;

export default {
  ReadableStream,
  WritableStream,
  TransformStream,
  CountQueuingStrategy,
  ByteLengthQueuingStrategy,
};
