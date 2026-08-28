// node:v8 — 堆统计最小真实面。
// 数据与 process.memoryUsage 同源（运行时 GC 的真实计量）：
// used_heap_size = 堆已用字节，heap_size_limit/total_heap_size = 堆上限。
// wjsm 未计量的维度（malloc 统计、global handle 等）如实报 0，不伪造数值。

const proc = globalThis.process;

export function getHeapStatistics() {
  const usage = proc.memoryUsage();
  const used = usage.heapUsed;
  const total = usage.heapTotal;
  return {
    total_heap_size: total,
    total_heap_size_executable: 0,
    total_physical_size: 0,
    total_available_size: total - used,
    used_heap_size: used,
    heap_size_limit: total,
    malloced_memory: 0,
    peak_malloced_memory: 0,
    does_zap_garbage: 0,
    number_of_native_contexts: 1,
    number_of_detached_contexts: 0,
    total_global_handles_size: 0,
    used_global_handles_size: 0,
    external_memory: usage.external,
  };
}

// 非目标：明确抛错，不留 no-op（与 node:vm 的处理一致）。
export function getHeapSpaceStatistics() {
  throw new Error('not implemented in wjsm: v8.getHeapSpaceStatistics');
}

export function getHeapCodeStatistics() {
  throw new Error('not implemented in wjsm: v8.getHeapCodeStatistics');
}

export function getHeapSnapshot() {
  throw new Error('not implemented in wjsm: v8.getHeapSnapshot');
}

export function writeHeapSnapshot() {
  throw new Error('not implemented in wjsm: v8.writeHeapSnapshot');
}

export function serialize() {
  throw new Error('not implemented in wjsm: v8.serialize');
}

export function deserialize() {
  throw new Error('not implemented in wjsm: v8.deserialize');
}

export function setFlagsFromString() {
  throw new Error('not implemented in wjsm: v8.setFlagsFromString');
}

export function cachedDataVersionTag() {
  throw new Error('not implemented in wjsm: v8.cachedDataVersionTag');
}

export default {
  getHeapStatistics,
  getHeapSpaceStatistics,
  getHeapCodeStatistics,
  getHeapSnapshot,
  writeHeapSnapshot,
  serialize,
  deserialize,
  setFlagsFromString,
  cachedDataVersionTag,
};
