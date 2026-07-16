const { parentPort, workerData } = require("node:worker_threads");

const histogram = workerData.histogram;
histogram.record(9);
parentPort.postMessage({
  graph:
    workerData.self === workerData &&
    histogram === workerData.repeated,
  recordable:
    histogram.constructor.name === "RecordableHistogram" &&
    typeof histogram.record === "function",
  intervalBase:
    workerData.interval.constructor.name === "Histogram" &&
    typeof workerData.interval.enable === "undefined" &&
    typeof workerData.interval.record === "undefined",
  workerCount: histogram.count,
});
