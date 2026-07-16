const {
  PerformanceObserver,
  performance,
} = require("node:perf_hooks");

performance.clearMarks();
performance.clearMeasures();

let callbackWasSynchronous = true;
let callbackCount = 0;
let observer;
observer = new PerformanceObserver((list, self) => {
  callbackCount += 1;
  const entries = list.getEntries();
  const marks = list.getEntriesByType("mark");
  console.log(
    !callbackWasSynchronous &&
      self === observer &&
      callbackCount === 1 &&
      entries.length === 3 &&
      entries[0].name === "measure" &&
      entries[1].name === "first" &&
      entries[2].name === "second" &&
      marks.length === 2 &&
      list.getEntriesByName("first", "mark")[0] === marks[0] &&
      observer.takeRecords().length === 0
  );

  observer.disconnect();
  performance.clearMarks();
  performance.clearMeasures();
  runBufferedObservation();
});

observer.observe({ entryTypes: ["mark", "measure"] });
performance.mark("taken", { startTime: 30 });
const taken = observer.takeRecords();
console.log(
  taken.length === 1 &&
    taken[0].name === "taken" &&
    taken[0].entryType === "mark"
);

performance.mark("second", { startTime: 20 });
performance.mark("first", { startTime: 10 });
performance.measure("measure", { start: 5, duration: 2 });
callbackWasSynchronous = false;

function runBufferedObservation() {
  performance.mark("history", { startTime: 1 });
  let bufferedWasSynchronous = true;
  let buffered;
  buffered = new PerformanceObserver((list, self) => {
    const entries = list.getEntries();
    console.log(
      !bufferedWasSynchronous &&
        self === buffered &&
        entries.length === 1 &&
        entries[0].name === "history" &&
        entries[0].entryType === "mark"
    );
    buffered.disconnect();
    runDisconnectChecks();
  });
  buffered.observe({ type: "mark", buffered: true });
  bufferedWasSynchronous = false;
}

function runDisconnectChecks() {
  let droppedCallbacks = 0;
  const dropped = new PerformanceObserver(() => {
    droppedCallbacks += 1;
  });
  dropped.observe({ entryTypes: ["mark"] });
  performance.mark("drop-me");
  dropped.disconnect();

  const reusable = new PerformanceObserver(() => {});
  reusable.observe({ type: "mark" });
  let modeChangeRejected = false;
  try {
    reusable.observe({ entryTypes: ["mark"] });
  } catch (error) {
    modeChangeRejected = error.name === "InvalidModificationError";
  }
  reusable.disconnect();

  let modeReset = true;
  try {
    reusable.observe({ entryTypes: ["mark"] });
  } catch (error) {
    modeReset = false;
  }
  reusable.disconnect();
  console.log(
    dropped.takeRecords().length === 0 && modeChangeRejected && modeReset
  );

  setImmediate(() => {
    console.log(droppedCallbacks === 0 && callbackCount === 1);
    performance.clearMarks();
    performance.clearMeasures();
  });
}
