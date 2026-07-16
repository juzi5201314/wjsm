const {
  PerformanceObserver,
  createHistogram,
  timerify,
} = require("node:perf_hooks");

const entries = [];
const observer = new PerformanceObserver((list) => {
  const batch = list.getEntriesByType("function");
  for (let index = 0; index < batch.length; index += 1) {
    entries.push(batch[index]);
  }
});
observer.observe({ entryTypes: ["function"] });

const histogram = createHistogram();
const argument = { value: 1 };
const inspect = timerify(function inspect(value) {
  return value;
}, { histogram });
console.log(inspect(argument) === argument);
argument.value = 2;

let synchronousThrow = false;
const throwing = timerify(function throwing() {
  throw new Error("expected");
});
try {
  throwing();
} catch (error) {
  synchronousThrow = error.message === "expected";
}

function Box(value) {
  this.value = value;
}
const TimedBox = timerify(Box);
const box = new TimedBox(7);
console.log(
  box instanceof Box && !(box instanceof TimedBox) && box.value === 7
);

const asyncValue = timerify(function asyncValue(value) {
  return new Promise((resolve) => {
    setImmediate(() => resolve(value * 2));
  });
}, { histogram });

const asyncReject = timerify(function asyncReject() {
  return Promise.reject(new Error("rejected"));
}, { histogram });

Promise.all([
  asyncValue(4),
  asyncReject().then(
    () => false,
    (error) => error.message === "rejected"
  ),
]).then((results) => {
  setImmediate(() => {
    setImmediate(() => {
      observer.disconnect();
      const names = entries.map((entry) => entry.name).sort().join(",");
      const inspectEntry = entries.find((entry) => entry.name === "inspect");
      console.log(results[0] === 8 && results[1] === true && synchronousThrow);
      console.log(
        entries.length === 4 &&
          names === "Box,asyncReject,asyncValue,inspect" &&
          entries.every(
            (entry) =>
              entry.entryType === "function" && entry.duration >= 0
          )
      );
      console.log(
        inspectEntry.detail.length === 1 &&
          inspectEntry.detail[0] === argument &&
          inspectEntry.detail[0].value === 2
      );
      console.log(
        histogram.count === 3 &&
          histogram.min > 0 &&
          histogram.max >= histogram.min &&
          histogram.exceeds === 0
      );
    });
  });
});
