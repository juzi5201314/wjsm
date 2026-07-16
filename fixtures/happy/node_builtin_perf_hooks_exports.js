const globalPerformance = globalThis.performance;
const bare = require("perf_hooks");
const hooks = require("node:perf_hooks");

const exportNames = [
  "Performance",
  "PerformanceEntry",
  "PerformanceMark",
  "PerformanceMeasure",
  "PerformanceObserver",
  "PerformanceObserverEntryList",
  "PerformanceResourceTiming",
  "constants",
  "createHistogram",
  "eventLoopUtilization",
  "monitorEventLoopDelay",
  "performance",
  "timerify",
];
console.log(Object.keys(hooks).sort().join(",") === exportNames.sort().join(","));
console.log(bare === hooks);
console.log(
  hooks.performance === globalPerformance &&
    hooks.performance === globalThis.performance &&
    hooks.performance instanceof hooks.Performance
);

const globalClasses = [
  "Performance",
  "PerformanceEntry",
  "PerformanceMark",
  "PerformanceMeasure",
  "PerformanceObserver",
  "PerformanceObserverEntryList",
  "PerformanceResourceTiming",
];
console.log(globalClasses.every((name) => globalThis[name] === hooks[name]));
console.log(
  hooks.timerify === hooks.performance.timerify &&
    hooks.eventLoopUtilization === hooks.performance.eventLoopUtilization
);

const supported = [
  "dns",
  "function",
  "gc",
  "http",
  "http2",
  "mark",
  "measure",
  "net",
  "resource",
];
console.log(
  Object.isFrozen(hooks.PerformanceObserver.supportedEntryTypes) &&
    hooks.PerformanceObserver.supportedEntryTypes.join(",") === supported.join(",")
);

const expectedConstants = {
  NODE_PERFORMANCE_GC_MINOR: 1,
  NODE_PERFORMANCE_GC_MAJOR: 4,
  NODE_PERFORMANCE_GC_INCREMENTAL: 8,
  NODE_PERFORMANCE_GC_WEAKCB: 16,
  NODE_PERFORMANCE_GC_FLAGS_NO: 0,
  NODE_PERFORMANCE_GC_FLAGS_CONSTRUCT_RETAINED: 2,
  NODE_PERFORMANCE_GC_FLAGS_FORCED: 4,
  NODE_PERFORMANCE_GC_FLAGS_SYNCHRONOUS_PHANTOM_PROCESSING: 8,
  NODE_PERFORMANCE_GC_FLAGS_ALL_AVAILABLE_GARBAGE: 16,
  NODE_PERFORMANCE_GC_FLAGS_ALL_EXTERNAL_MEMORY: 32,
  NODE_PERFORMANCE_GC_FLAGS_SCHEDULE_IDLE: 64,
};
console.log(
  Object.keys(hooks.constants).length === Object.keys(expectedConstants).length &&
    Object.keys(expectedConstants).every(
      (name) => hooks.constants[name] === expectedConstants[name]
    )
);

const before = hooks.performance.now();
const wallClockDelta = Math.abs(
  hooks.performance.timeOrigin + before - Date.now()
);
setImmediate(() => {
  const after = hooks.performance.now();
  console.log(
    Number.isFinite(before) &&
      Number.isFinite(after) &&
      after >= before &&
      hooks.performance.timeOrigin > 0 &&
      wallClockDelta < 5000
  );
});
