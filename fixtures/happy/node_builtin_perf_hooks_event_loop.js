const {
  eventLoopUtilization,
  monitorEventLoopDelay,
  performance,
} = require("node:perf_hooks");

const initialUtilization = eventLoopUtilization();
const delay = monitorEventLoopDelay({ resolution: 1 });
const firstEnable = delay.enable();
const secondEnable = delay.enable();

setTimeout(() => {
  const utilization = eventLoopUtilization();
  const delta = eventLoopUtilization(utilization, initialUtilization);
  const timing = performance.nodeTiming;
  const metrics = timing.uvMetricsInfo;
  const sampledCount = delay.count;
  const firstDisable = delay.disable();
  const secondDisable = delay.disable();

  console.log(
    timing.name === "node" &&
      timing.entryType === "node" &&
      timing.startTime === 0 &&
      timing.duration >= timing.bootstrapComplete &&
      timing.nodeStart >= 0 &&
      timing.v8Start >= timing.nodeStart &&
      timing.environment >= timing.v8Start &&
      timing.bootstrapComplete >= timing.environment &&
      timing.loopStart >= 0 &&
      timing.loopExit === -1 &&
      timing.idleTime >= 0 &&
      performance.getEntriesByType("node").length === 0
  );
  console.log(
    Number.isInteger(metrics.loopCount) &&
      metrics.loopCount >= 1 &&
      Number.isInteger(metrics.events) &&
      metrics.events >= 0 &&
      Number.isInteger(metrics.eventsWaiting) &&
      metrics.eventsWaiting >= 0
  );
  console.log(
    utilization.idle >= 0 &&
      utilization.active >= 0 &&
      utilization.utilization >= 0 &&
      utilization.utilization <= 1 &&
      delta.idle >= 0 &&
      delta.active >= 0 &&
      delta.utilization >= 0 &&
      delta.utilization <= 1
  );
  console.log(
    firstEnable === true &&
      secondEnable === false &&
      firstDisable === true &&
      secondDisable === false &&
      typeof delay.record === "undefined" &&
      sampledCount > 0 &&
      delay.min > 0 &&
      delay.max >= delay.min &&
      delay.mean >= 0 &&
      delay.stddev >= 0 &&
      delay.percentile(50) > 0
  );

  setTimeout(() => {
    console.log(delay.count === sampledCount);
  }, 5);
}, 25);
