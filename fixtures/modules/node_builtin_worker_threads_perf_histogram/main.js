const { createHistogram, monitorEventLoopDelay } = require("node:perf_hooks");
const { Worker } = require("node:worker_threads");
const path = require("node:path");

const histogram = createHistogram();
histogram.record(7);
const interval = monitorEventLoopDelay({ resolution: 1000 });
const workerData = {
  histogram,
  repeated: histogram,
  interval,
};
workerData.self = workerData;

const worker = new Worker(path.join(__dirname, "worker.js"), { workerData });
worker.on("message", (message) => {
  console.log(
    message.graph &&
      message.recordable &&
      message.intervalBase &&
      message.workerCount === 2 &&
      histogram.count === 2
  );
  worker.terminate();
});
worker.on("exit", () => {
  process.exit(0);
});
