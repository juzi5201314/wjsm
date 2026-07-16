const {
  PerformanceObserver,
  PerformanceEntry,
  constants,
} = require('node:perf_hooks');

let observer;
observer = new PerformanceObserver((list) => {
  const forced = list.getEntries().find(
    (entry) => entry.detail.flags === constants.NODE_PERFORMANCE_GC_FLAGS_FORCED
  );
  if (!forced) return;
  console.log(Boolean(
    forced instanceof PerformanceEntry &&
    forced.name === 'gc' &&
    forced.entryType === 'gc' &&
    forced.detail.kind === constants.NODE_PERFORMANCE_GC_MAJOR &&
    forced.startTime >= 0 &&
    forced.duration >= 0 &&
    forced.toJSON().detail === forced.detail
  ));
  observer.disconnect();
});

observer.observe({ type: 'gc' });
globalThis.gc();
