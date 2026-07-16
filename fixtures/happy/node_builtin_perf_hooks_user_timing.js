const {
  PerformanceMark,
  PerformanceMeasure,
  performance,
} = require("node:perf_hooks");

performance.clearMarks();
performance.clearMeasures();

const markDetail = { nested: { value: 1 } };
const mark = performance.mark("alpha", {
  startTime: 5,
  detail: markDetail,
});
markDetail.nested.value = 2;
console.log(
  mark instanceof PerformanceMark &&
    mark.name === "alpha" &&
    mark.entryType === "mark" &&
    mark.startTime === 5 &&
    mark.duration === 0 &&
    mark.detail.nested.value === 1
);

const constructorDetail = { value: 3 };
const constructorMark = new PerformanceMark("constructor-mark", {
  startTime: 7,
  detail: constructorDetail,
});
constructorDetail.value = 4;
console.log(
  constructorMark.detail.value === 3 &&
    performance.getEntriesByName("constructor-mark").length === 0
);

const measureDetail = { nested: { value: 5 } };
const measure = performance.measure("span", {
  start: "constructor-mark",
  end: 12,
  detail: measureDetail,
});
measureDetail.nested.value = 6;
console.log(
  measure instanceof PerformanceMeasure &&
    measure.entryType === "measure" &&
    measure.startTime === 7 &&
    measure.duration === 5 &&
    measure.detail.nested.value === 5
);

console.log(
  performance.getEntriesByType("mark").length === 1 &&
    performance.getEntriesByType("measure").length === 1 &&
    performance.getEntriesByName("span", "measure")[0] === measure
);

performance.clearMarks("alpha");
performance.clearMeasures("span");
console.log(
  performance.getEntriesByType("mark").length === 0 &&
    performance.getEntriesByType("measure").length === 0
);

performance.mark(123);
let invalidMeasureName = false;
let missingMark = false;
let reservedMark = false;
try {
  performance.measure(123);
} catch (error) {
  invalidMeasureName =
    error.name === "TypeError" && error.code === "ERR_INVALID_ARG_TYPE";
}
try {
  performance.measure("missing", "does-not-exist");
} catch (error) {
  missingMark = error.name === "SyntaxError" && error.code === 12;
}
try {
  performance.mark("nodeStart");
} catch (error) {
  reservedMark =
    error.name === "TypeError" && error.code === "ERR_INVALID_ARG_VALUE";
}
console.log(
  performance.getEntriesByName("123", "mark").length === 1 &&
    invalidMeasureName &&
    missingMark &&
    reservedMark
);

performance.clearMarks();
performance.clearMeasures();
