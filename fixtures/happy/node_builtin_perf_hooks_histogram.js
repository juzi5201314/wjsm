const { createHistogram } = require("node:perf_hooks");

const histogram = createHistogram({ lowest: 1, highest: 100, figures: 3 });
console.log(
  histogram.count === 0 &&
    histogram.countBigInt === 0n &&
    histogram.minBigInt === 9223372036854775807n &&
    histogram.maxBigInt === 0n &&
    histogram.exceedsBigInt === 0n
);

histogram.record(10);
histogram.record(20n);
histogram.record(1000);
console.log(
  histogram.count === 2 &&
    histogram.countBigInt === 2n &&
    histogram.min === 10 &&
    histogram.max === 20 &&
    histogram.exceeds === 1 &&
    histogram.exceedsBigInt === 1n &&
    histogram.percentile(50) === 10 &&
    histogram.percentileBigInt(50) === 10n &&
    histogram.percentiles.get(50) === 10 &&
    histogram.percentilesBigInt.get(50) === 10n
);

const other = createHistogram({ lowest: 1, highest: 100, figures: 3 });
other.record(30);
histogram.add(other);
const json = histogram.toJSON();
console.log(
  histogram.count === 3 &&
    histogram.min === 10 &&
    histogram.max === 30 &&
    histogram.mean === 20 &&
    histogram.stddev > 0 &&
    histogram.percentile(50) === 20 &&
    json.count === 3 &&
    json.min === 10 &&
    json.max === 30 &&
    json.exceeds === 1 &&
    json.percentiles[50] === 20
);

histogram.reset();
console.log(
  histogram.count === 0 &&
    histogram.exceeds === 0 &&
    histogram.minBigInt === 9223372036854775807n &&
    histogram.maxBigInt === 0n
);

const delta = createHistogram();
delta.recordDelta();
setTimeout(() => {
  delta.recordDelta();
  console.log(
    delta.count === 1 &&
      delta.min > 0 &&
      delta.max >= delta.min &&
      delta.exceeds === 0
  );
}, 5);
