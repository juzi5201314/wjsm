const { performance } = require("node:perf_hooks");

performance.clearMarks();
performance.clearMeasures();
performance.clearResourceTimings();

const mark = performance.mark("timeline-mark", { startTime: 3 });
const measure = performance.measure("timeline-measure", {
  start: 3,
  end: 5,
});

console.log(
  performance.getEntriesByType("mark").length === 1 &&
    performance.getEntriesByType("mark")[0] === mark
);
console.log(
  performance.getEntriesByType("measure").length === 1 &&
    performance.getEntriesByType("measure")[0] === measure
);
console.log(
  performance.getEntriesByName("timeline-mark", "mark")[0] === mark &&
    performance.getEntriesByName("timeline-measure", "measure")[0] === measure
);
console.log(
  performance.getEntries().length === 2 &&
    performance.getEntries()[0] === mark &&
    performance.getEntries()[1] === measure
);

performance.clearMarks("timeline-mark");
performance.clearMeasures("timeline-measure");
console.log(
  performance.getEntriesByType("mark").length === 0 &&
    performance.getEntriesByType("measure").length === 0
);

performance.mark(123, { startTime: 7 });
console.log(performance.getEntriesByName("123", "mark").length === 1);
performance.clearMarks();
