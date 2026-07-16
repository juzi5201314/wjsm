const originalStructuredClone = globalThis.structuredClone;
let replacementCalls = 0;
globalThis.structuredClone = () => {
  replacementCalls += 1;
  return { replaced: true };
};

const { performance } = require("node:perf_hooks");
const markSource = { nested: { value: 1 } };
const measureSource = { nested: { value: 2 } };
const mark = performance.mark("primordial-mark", { detail: markSource });
const measure = performance.measure("primordial-measure", {
  start: 0,
  end: 1,
  detail: measureSource,
});
globalThis.structuredClone = originalStructuredClone;
markSource.nested.value = 3;
measureSource.nested.value = 4;

console.log(
  replacementCalls === 0 &&
    mark.detail.nested.value === 1 &&
    measure.detail.nested.value === 2
);

performance.clearMarks();
performance.clearMeasures();
