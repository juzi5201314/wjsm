const { createHook, asyncWrapProviders } = require('node:async_hooks');
const {
  eventLoopUtilization,
  monitorEventLoopDelay,
} = require('node:perf_hooks');

let initialized = 0;
let histogramResources = 0;
let destroyed = 0;
const histogramIds = new Set();
const hook = createHook({
  init(asyncId, type, triggerAsyncId, resource) {
    if (type !== 'ELDHISTOGRAM') return;
    initialized++;
    histogramIds.add(asyncId);
    if (typeof resource.percentile === 'function') {
      histogramResources++;
    }
  },
  destroy(asyncId) {
    if (histogramIds.has(asyncId)) destroyed++;
  },
}).enable();

let ignoredOptions = 0;
for (const samplePerIteration of [null, 'yes', 1, {}, []]) {
  try {
    const probe = monitorEventLoopDelay({ samplePerIteration });
    if (typeof probe.enable === 'function') ignoredOptions++;
  } catch (error) {}
}

let interval = monitorEventLoopDelay({ resolution: 60_000 });
let ignoredOption = monitorEventLoopDelay({
  resolution: 60_000,
  samplePerIteration: true,
});

console.log(ignoredOptions === 5);
console.log(asyncWrapProviders.ELDHISTOGRAM === 3);
console.log(initialized === 7 && histogramResources === 7);
console.log(interval.enable() && ignoredOption.enable());

setTimeout(() => {
  const intervalCount = interval.count;
  const ignoredOptionCount = ignoredOption.count;
  const firstDisable = interval.disable();
  const secondDisable = interval.disable();
  ignoredOption.disable();
  console.log(
    intervalCount === 0 &&
      ignoredOptionCount === 0 &&
      firstDisable === true &&
      secondDisable === false
  );

  const zeroDelta = eventLoopUtilization(
    { idle: 0, active: 0 },
    { idle: 0, active: 0 }
  );
  console.log(Number.isNaN(zeroDelta.utilization));

  interval = null;
  ignoredOption = null;
  setImmediate(() => {
    gc();
    setTimeout(() => {
      setTimeout(() => {
        hook.disable();
        console.log(destroyed === 7);
      }, 0);
    }, 0);
  });
}, 5);
