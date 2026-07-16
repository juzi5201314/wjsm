const cloneBeforePerfHooks = globalThis.structuredClone;
const {
  createHistogram,
  monitorEventLoopDelay,
} = require("node:perf_hooks");

console.log(cloneBeforePerfHooks === globalThis.structuredClone);
console.log(createHistogram.length === 0 && monitorEventLoopDelay.length === 0);

const histogram = createHistogram({ lowest: 1, highest: 1000, figures: 3 });
histogram.record(42);
const graph = {
  histogram,
  nested: { histogram },
};
graph.self = graph;

const clonedGraph = structuredClone(graph);
console.log(
  clonedGraph !== graph &&
    clonedGraph.self === clonedGraph &&
    clonedGraph.histogram === clonedGraph.nested.histogram &&
    clonedGraph.histogram.constructor.name === "RecordableHistogram" &&
    typeof clonedGraph.histogram.record === "function" &&
    clonedGraph.histogram.count === 1
);

clonedGraph.histogram.record(43);
console.log(histogram.count === 2 && clonedGraph.histogram.count === 2);

const child = Object.create(histogram);
const transparentProxy = new Proxy(histogram, {});
child.record(44);
console.log(
  child.count === 3 &&
    child.percentile(50) > 0
);
histogram.record.call(transparentProxy, 45);
const countGetter = Object.getOwnPropertyDescriptor(
  Object.getPrototypeOf(Object.getPrototypeOf(histogram)),
  "count"
).get;
console.log(
  countGetter.call(transparentProxy) === 4 &&
    histogram.percentile.call(transparentProxy, 50) > 0
);
const clonedChild = structuredClone(child);
let proxyCloneRejected = false;
try {
  structuredClone(transparentProxy);
} catch (error) {
  proxyCloneRejected = error.name === "DataCloneError";
}
const hidingProxy = new Proxy(histogram, {
  get() { return undefined; },
});
let hidingProxyRejected = false;
try {
  histogram.percentile.call(hidingProxy, 50);
} catch (error) {
  hidingProxyRejected = error.code === "ERR_INVALID_THIS";
}
console.log(
  Object.getPrototypeOf(clonedChild) !== Object.getPrototypeOf(histogram)
);
console.log(clonedChild.count === undefined);
console.log(proxyCloneRejected);
console.log(hidingProxyRejected);

const interval = monitorEventLoopDelay({ resolution: 1000 });
const basePrototype = Object.getPrototypeOf(Object.getPrototypeOf(histogram));
const constructors = [
  [basePrototype.constructor, "Histogram"],
  [histogram.constructor, "RecordableHistogram"],
  [interval.constructor, "ELDHistogram"],
];
let constructorNames = true;
let constructorPrototypeDescriptors = true;
let ownConstructorDescriptors = true;
for (let constructorIndex = 0; constructorIndex < constructors.length; constructorIndex++) {
  const Constructor = constructors[constructorIndex][0];
  const name = constructors[constructorIndex][1];
  constructorNames = constructorNames &&
    Constructor.name === name && Constructor.length === 0;
    const prototype = Object.getOwnPropertyDescriptor(Constructor, "prototype");
  constructorPrototypeDescriptors = constructorPrototypeDescriptors &&
      prototype.writable === false &&
      prototype.enumerable === false &&
      prototype.configurable === false &&
      prototype.value === Constructor.prototype;
    const ownConstructor = Object.getOwnPropertyDescriptor(
      Constructor.prototype,
      "constructor"
    );
  ownConstructorDescriptors = ownConstructorDescriptors &&
      ownConstructor.value === Constructor &&
      ownConstructor.writable === true &&
      ownConstructor.enumerable === false &&
      ownConstructor.configurable === true;
}
console.log(constructorNames);
console.log(constructorPrototypeDescriptors);
console.log(ownConstructorDescriptors);
console.log(Object.getPrototypeOf(Object.getPrototypeOf(interval)) === basePrototype);
const accessorShape = [
  "count",
  "countBigInt",
  "min",
  "minBigInt",
  "max",
  "maxBigInt",
  "mean",
  "stddev",
  "exceeds",
  "exceedsBigInt",
  "percentiles",
  "percentilesBigInt",
].every((name) => {
  const descriptor = Object.getOwnPropertyDescriptor(basePrototype, name);
  return (
    typeof descriptor.get === "function" &&
    descriptor.set === undefined &&
    descriptor.enumerable === false &&
    descriptor.configurable === true
  );
});
const methodSpecs = [
  [basePrototype, "percentile", 1],
  [basePrototype, "percentileBigInt", 1],
  [basePrototype, "reset", 0],
  [basePrototype, "toJSON", 0],
  [Object.getPrototypeOf(histogram), "record", 1],
  [Object.getPrototypeOf(histogram), "recordDelta", 0],
  [Object.getPrototypeOf(histogram), "add", 1],
  [Object.getPrototypeOf(interval), "enable", 0],
  [Object.getPrototypeOf(interval), "disable", 0],
];
let methodDescriptors = true;
let methodLengths = true;
for (let methodIndex = 0; methodIndex < methodSpecs.length; methodIndex++) {
  const prototype = methodSpecs[methodIndex][0];
  const name = methodSpecs[methodIndex][1];
  const length = methodSpecs[methodIndex][2];
  const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
  methodDescriptors = methodDescriptors &&
    typeof descriptor.value === "function" &&
    descriptor.writable === true &&
    descriptor.enumerable === false &&
    descriptor.configurable === true;
}
methodLengths =
  Object.getOwnPropertyDescriptor(basePrototype, "percentile").value.length === 1 &&
  Object.getOwnPropertyDescriptor(basePrototype, "percentileBigInt").value.length === 1 &&
  Object.getOwnPropertyDescriptor(basePrototype, "reset").value.length === 0 &&
  Object.getOwnPropertyDescriptor(basePrototype, "toJSON").value.length === 0 &&
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(histogram), "record").value.length === 1 &&
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(histogram), "recordDelta").value.length === 0 &&
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(histogram), "add").value.length === 1 &&
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(interval), "enable").value.length === 0 &&
  Object.getOwnPropertyDescriptor(Object.getPrototypeOf(interval), "disable").value.length === 0;
const disposeShape = Symbol.dispose === undefined || (() => {
  const descriptor = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(interval),
    Symbol.dispose
  );
  return (
    typeof descriptor.value === "function" &&
    descriptor.value.length === 0 &&
    descriptor.writable === true &&
    descriptor.enumerable === false &&
    descriptor.configurable === true
  );
})();
console.log(accessorShape);
console.log(methodDescriptors);
console.log(methodLengths);
console.log(disposeShape);
const clonedInterval = structuredClone(interval);
console.log(
  clonedInterval.constructor.name === "Histogram" &&
    typeof clonedInterval.enable === "undefined" &&
    typeof clonedInterval.record === "undefined" &&
    clonedInterval.count === 0
);

const fake = Object.create(Object.getPrototypeOf(histogram));
fake.__wjsm_perf_histogram_handle__ = 0;
fake.__wjsm_perf_histogram_kind__ = 1;
Object.prototype.__wjsm_perf_histogram_handle__ = 0;
Object.prototype.__wjsm_perf_histogram_kind__ = 1;
let fakeRejected = false;
try {
  histogram.percentile.call(fake, 50);
} catch (error) {
  fakeRejected = error.code === "ERR_INVALID_THIS";
}
delete Object.prototype.__wjsm_perf_histogram_handle__;
delete Object.prototype.__wjsm_perf_histogram_kind__;
console.log(fakeRejected && histogram.count === 4);

const buffer = new ArrayBuffer(4);
const transferGraph = {
  histogram,
  repeated: histogram,
  buffer,
};
transferGraph.self = transferGraph;
const transferred = structuredClone(transferGraph, { transfer: [buffer] });
console.log(
  buffer.byteLength === 0 &&
    transferred.buffer.byteLength === 4 &&
    transferred.self === transferred &&
    transferred.histogram === transferred.repeated &&
    transferred.histogram.constructor.name === "RecordableHistogram"
);

const narrow = createHistogram({ lowest: 1, highest: 100, figures: 3 });
const wide = createHistogram({ lowest: 1, highest: 1000, figures: 3 });
wide.record(500);
wide.record(2000);
narrow.add(wide);
console.log(narrow.count === 1 && narrow.max === 0 && narrow.exceeds === 1);

let mixedTypeErrors = 0;
for (const options of [
  { lowest: 1n },
  { lowest: 1n, highest: 10 },
  { lowest: 1, highest: 10n },
]) {
  try {
    createHistogram(options);
  } catch (error) {
    if (error.name === "TypeError") mixedTypeErrors++;
  }
}
let safeBigIntRangeError = false;
try {
  createHistogram({ lowest: 0n, highest: 10n });
} catch (error) {
  safeBigIntRangeError = error.code === "ERR_OUT_OF_RANGE";
}
const bigintConfigured = createHistogram({ lowest: 1n, highest: 10n });
console.log(
  mixedTypeErrors === 3 &&
    safeBigIntRangeError &&
    bigintConfigured.count === 0
);

const bigint = createHistogram();
bigint.record(9223372036854775807n);
let bigintRejected = false;
try {
  bigint.record(9223372036854775808n);
} catch (error) {
  bigintRejected = error.code === "ERR_OUT_OF_RANGE";
}
console.log(bigint.count === 0 && bigint.exceeds === 1 && bigintRejected);

gc();
console.log(
  histogram.countBigInt === 4n &&
    histogram.percentilesBigInt.get(100) >= 45n
);
