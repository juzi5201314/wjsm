// HDR Histogram 公共 wrapper；样本存储和 percentile 计算由 host 单一持有。

const HISTOGRAM_BASE = 0;
const HISTOGRAM_RECORDABLE = 1;
const HISTOGRAM_INTERVAL = 2;
const MAX_SAFE_INTEGER = 9007199254740991;
const MAX_U64 = 18446744073709551615n;
const MAX_RECORD_VALUE = 9223372036854775807n;

var Histogram;
var RecordableHistogram;
var IntervalHistogram;
var installHistogramInheritance;
var initializeHistogram;
var createHistogramWrapper;
var isHistogram;
var histogramKind;
var requireHistogram;
var histogramStats;
var installHistogramAccessors;
var validatePercentile;
var installHistogramPercentileMethods;
var fillPercentileMap;
var installHistogramPercentileAccessors;
var installHistogramBaseMethods;
var validateRecordValue;
var installRecordableHistogramMethods;
var installIntervalHistogramMethods;
var createHistogram;
var monitorEventLoopDelay;
var installHistogram;

function loadPerfHooksHistogram() {

function Histogram() {
  throw illegalConstructor();
}

function RecordableHistogram() {
  throw illegalConstructor();
}

function ELDHistogram() {
  throw illegalConstructor();
}

function installHistogramInheritance() {
Object.defineProperty(Histogram.prototype, 'constructor', {
  value: Histogram,
  writable: true,
  enumerable: false,
  configurable: true,
});

RecordableHistogram.prototype = Object.create(Histogram.prototype);
Object.defineProperty(RecordableHistogram.prototype, 'constructor', {
  value: RecordableHistogram,
  writable: true,
  enumerable: false,
  configurable: true,
});

ELDHistogram.prototype = Object.create(Histogram.prototype);
Object.defineProperty(ELDHistogram.prototype, 'constructor', {
  value: ELDHistogram,
  writable: true,
  enumerable: false,
  configurable: true,
});

for (const Constructor of [Histogram, RecordableHistogram, ELDHistogram]) {
  Object.defineProperty(Constructor, 'prototype', {
    configurable: false,
    enumerable: false,
    writable: false,
    value: Constructor.prototype,
  });
}
}

function initializeHistogram(target, kind) {
  Object.defineProperties(target, {
    [kHistogramBrand]: { value: true },
    [kHistogramKind]: { value: kind },
    [kHistogramMap]: { value: new Map() },
  });
  return target;
}

function createHistogramWrapper(histogram, kind) {
  initializeHistogram(histogram, kind);
  if (kind === HISTOGRAM_INTERVAL) {
    Object.defineProperties(histogram, {
      [kIntervalEnabled]: { value: false, writable: true },
      [kIntervalResolution]: { value: 10, writable: true },
    });
  }
  return histogram;
}

function isHistogram(value) {
  return value !== null && value !== undefined && perfHost.histogramKind(value) >= 0;
}

function histogramKind(value) {
  return perfHost.histogramKind(value);
}

function requireHistogram(value, name) {
  const kind = histogramKind(value);
  if (kind < 0) throw invalidThis(name || 'Histogram');
  if (value[kHistogramBrand] !== true) {
    Object.defineProperties(value, {
      [kHistogramBrand]: { value: true },
      [kHistogramKind]: { value: kind },
      [kHistogramMap]: { value: new Map() },
    });
  }
  return kind;
}

function histogramStats(value) {
  requireHistogram(value, 'Histogram');
  return perfHost.histogramStats(value);
}

function installHistogramAccessors() {
Object.defineProperties(Histogram.prototype, {
  count: {
    configurable: true,
    get: function () { return histogramStats(this).count; },
  },
  countBigInt: {
    configurable: true,
    get: function () { return histogramStats(this).countBigInt; },
  },
  min: {
    configurable: true,
    get: function () { return histogramStats(this).min; },
  },
  minBigInt: {
    configurable: true,
    get: function () { return histogramStats(this).minBigInt; },
  },
  max: {
    configurable: true,
    get: function () { return histogramStats(this).max; },
  },
  maxBigInt: {
    configurable: true,
    get: function () { return histogramStats(this).maxBigInt; },
  },
  mean: {
    configurable: true,
    get: function () { return histogramStats(this).mean; },
  },
  stddev: {
    configurable: true,
    get: function () { return histogramStats(this).stddev; },
  },
  exceeds: {
    configurable: true,
    get: function () { return histogramStats(this).exceeds; },
  },
  exceedsBigInt: {
    configurable: true,
    get: function () { return histogramStats(this).exceedsBigInt; },
  },
});
}

function validatePercentile(percentile) {
  validateNumber(percentile, 'percentile');
  if (Number.isNaN(percentile) || percentile <= 0 || percentile > 100) {
    throw outOfRange('percentile', '> 0 && <= 100', percentile);
  }
}

function installHistogramPercentileMethods() {
Object.defineProperties(Histogram.prototype, {
  percentile: {
    configurable: true,
    writable: true,
    value: function percentile(requestedPercentile) {
      requireHistogram(this, 'Histogram');
      validatePercentile(requestedPercentile);
      return perfHost.histogramPercentile(this, requestedPercentile, false);
    },
  },
  percentileBigInt: {
    configurable: true,
    writable: true,
    value: function percentileBigInt(requestedPercentile) {
      requireHistogram(this, 'Histogram');
      validatePercentile(requestedPercentile);
      return perfHost.histogramPercentile(this, requestedPercentile, true);
    },
  },
});
}

function fillPercentileMap(histogram, bigint) {
  requireHistogram(histogram, 'Histogram');
  let map = histogram[kHistogramMap];
  if (map === undefined) {
    map = new Map();
    Object.defineProperty(histogram, kHistogramMap, { value: map });
  }
  map.clear();
  const flat = perfHost.histogramPercentiles(histogram, bigint);
  for (let i = 0; i + 1 < flat.length; i = i + 2) {
    map.set(flat[i], flat[i + 1]);
  }
  return map;
}

function installHistogramPercentileAccessors() {
Object.defineProperties(Histogram.prototype, {
  percentiles: {
    configurable: true,
    get: function () { return fillPercentileMap(this, false); },
  },
  percentilesBigInt: {
    configurable: true,
    get: function () { return fillPercentileMap(this, true); },
  },
});
}

function installHistogramBaseMethods() {
Object.defineProperties(Histogram.prototype, {
  reset: {
    configurable: true,
    writable: true,
    value: function reset() {
      requireHistogram(this, 'Histogram');
      perfHost.histogramReset(this);
    },
  },
  toJSON: {
    configurable: true,
    writable: true,
    value: function toJSON() {
      requireHistogram(this, 'Histogram');
      const percentiles = {};
      this.percentiles.forEach(function (value, percentile) {
        percentiles[percentile] = value;
      });
      return {
        count: this.count,
        min: this.min,
        max: this.max,
        mean: this.mean,
        exceeds: this.exceeds,
        stddev: this.stddev,
        percentiles: percentiles,
      };
    },
  },
});
}

function validateRecordValue(value) {
  if (typeof value === 'bigint') {
    if (value < 1n || value > MAX_RECORD_VALUE) {
      throw outOfRange('val', '>= 1 && <= 2 ** 63 - 1', value);
    }
    return;
  }
  validateInteger(value, 'val', 1, MAX_SAFE_INTEGER);
}

function installRecordableHistogramMethods() {
Object.defineProperties(RecordableHistogram.prototype, {
  record: {
    configurable: true,
    writable: true,
    value: function record(value) {
      if (requireHistogram(this, 'RecordableHistogram') !== HISTOGRAM_RECORDABLE) {
        throw invalidThis('RecordableHistogram');
      }
      validateRecordValue(value);
      perfHost.histogramRecord(this, value);
    },
  },
  recordDelta: {
    configurable: true,
    writable: true,
    value: function recordDelta() {
      if (requireHistogram(this, 'RecordableHistogram') !== HISTOGRAM_RECORDABLE) {
        throw invalidThis('RecordableHistogram');
      }
      perfHost.histogramRecordDelta(this);
    },
  },
  add: {
    configurable: true,
    writable: true,
    value: function add(other) {
      if (requireHistogram(this, 'RecordableHistogram') !== HISTOGRAM_RECORDABLE) {
        throw invalidThis('RecordableHistogram');
      }
      if (!isHistogram(other) || histogramKind(other) !== HISTOGRAM_RECORDABLE) {
        throw invalidType('other', 'RecordableHistogram', other);
      }
      perfHost.histogramAdd(this, other);
    },
  },
});
}

function installIntervalHistogramMethods() {
Object.defineProperties(ELDHistogram.prototype, {
  enable: {
    configurable: true,
    writable: true,
    value: function enable() {
      if (requireHistogram(this, 'ELDHistogram') !== HISTOGRAM_INTERVAL) {
        throw invalidThis('ELDHistogram');
      }
      if (this[kIntervalEnabled]) return false;
      const enabled = perfHost.eventLoopDelayEnable(this);
      if (enabled) this[kIntervalEnabled] = true;
      return Boolean(enabled);
    },
  },
  disable: {
    configurable: true,
    writable: true,
    value: function disable() {
      if (requireHistogram(this, 'ELDHistogram') !== HISTOGRAM_INTERVAL) {
        throw invalidThis('ELDHistogram');
      }
      if (!this[kIntervalEnabled]) return false;
      const disabled = perfHost.eventLoopDelayDisable(this);
      if (disabled) this[kIntervalEnabled] = false;
      return Boolean(disabled);
    },
  },
});

if (Symbol.dispose !== undefined) {
  Object.defineProperty(ELDHistogram.prototype, Symbol.dispose, {
    configurable: true,
    writable: true,
    value: function dispose() { this.disable(); },
  });
}
}

function createHistogram(options = undefined) {
  if (options === undefined) options = {};
  validateObject(options, 'options');
  const lowest = options.lowest === undefined ? 1 : options.lowest;
  const highest = options.highest === undefined ? MAX_SAFE_INTEGER : options.highest;
  const figures = options.figures === undefined ? 3 : options.figures;

  if (typeof lowest !== 'bigint') {
    validateInteger(lowest, 'options.lowest', 1, MAX_SAFE_INTEGER);
  } else if (lowest < 1n || lowest > MAX_U64) {
    throw outOfRange('options.lowest', '>= 1', lowest);
  }
  if (typeof highest !== 'bigint') {
    const minimum = 2 * lowest;
    validateInteger(highest, 'options.highest', minimum, MAX_SAFE_INTEGER);
  } else {
    const minimum = 2n * lowest;
    if (highest >= minimum && highest <= MAX_U64) {
      // 有效 BigInt 区间由 host 以 u64 精确接收。
    } else {
    throw makeError(
      RangeError,
      'ERR_INVALID_ARG_VALUE',
      'The argument options.highest is invalid'
    );
    }
  }
  validateInteger(figures, 'options.figures', 1, 5);
  const histogram = perfHost.histogramCreate(lowest, highest, figures);
  return createHistogramWrapper(histogram, HISTOGRAM_RECORDABLE);
}

function monitorEventLoopDelay(options = undefined) {
  if (options === undefined) options = {};
  validateObject(options, 'options');
  const resolution = options.resolution === undefined ? 10 : options.resolution;
  validateInteger(resolution, 'options.resolution', 1, MAX_SAFE_INTEGER);
  const histogram = createHistogramWrapper(
    perfHost.eventLoopDelayCreate(resolution),
    HISTOGRAM_INTERVAL
  );
  histogram[kIntervalResolution] = resolution;
  const asyncHooksHost = globalThis.__wjsm_node_async_hooks;
  if (!asyncHooksHost || typeof asyncHooksHost.asyncResourceInit !== 'function') {
    throw new Error('wjsm internal async_hooks host bridge is not installed');
  }
  asyncHooksHost.asyncResourceInit(histogram, 'ELDHISTOGRAM');
  return histogram;
}

function installHistogram() {
  for (const callable of [createHistogram, monitorEventLoopDelay]) {
    Object.defineProperty(callable, 'length', {
      configurable: true,
      enumerable: false,
      writable: false,
      value: 0,
    });
  }
  installHistogramInheritance();
  installHistogramAccessors();
  installHistogramPercentileMethods();
  installHistogramPercentileAccessors();
  installHistogramBaseMethods();
  installRecordableHistogramMethods();
  installIntervalHistogramMethods();
  if (typeof perfHost.registerHistogramPrototypes === 'function') {
    perfHost.registerHistogramPrototypes(
      Histogram.prototype,
      RecordableHistogram.prototype,
      ELDHistogram.prototype
    );
  }
}

  return {
    Histogram: Histogram,
    RecordableHistogram: RecordableHistogram,
    IntervalHistogram: ELDHistogram,
    installHistogramInheritance: installHistogramInheritance,
    initializeHistogram: initializeHistogram,
    createHistogramWrapper: createHistogramWrapper,
    isHistogram: isHistogram,
    histogramKind: histogramKind,
    requireHistogram: requireHistogram,
    histogramStats: histogramStats,
    installHistogramAccessors: installHistogramAccessors,
    validatePercentile: validatePercentile,
    installHistogramPercentileMethods: installHistogramPercentileMethods,
    fillPercentileMap: fillPercentileMap,
    installHistogramPercentileAccessors: installHistogramPercentileAccessors,
    installHistogramBaseMethods: installHistogramBaseMethods,
    validateRecordValue: validateRecordValue,
    installRecordableHistogramMethods: installRecordableHistogramMethods,
    installIntervalHistogramMethods: installIntervalHistogramMethods,
    createHistogram: createHistogram,
    monitorEventLoopDelay: monitorEventLoopDelay,
    installHistogram: installHistogram,
  };
}

let loadedPerfHooksHistogram = loadPerfHooksHistogram();
Histogram = loadedPerfHooksHistogram.Histogram;
RecordableHistogram = loadedPerfHooksHistogram.RecordableHistogram;
IntervalHistogram = loadedPerfHooksHistogram.IntervalHistogram;
installHistogramInheritance = loadedPerfHooksHistogram.installHistogramInheritance;
initializeHistogram = loadedPerfHooksHistogram.initializeHistogram;
createHistogramWrapper = loadedPerfHooksHistogram.createHistogramWrapper;
isHistogram = loadedPerfHooksHistogram.isHistogram;
histogramKind = loadedPerfHooksHistogram.histogramKind;
requireHistogram = loadedPerfHooksHistogram.requireHistogram;
histogramStats = loadedPerfHooksHistogram.histogramStats;
installHistogramAccessors = loadedPerfHooksHistogram.installHistogramAccessors;
validatePercentile = loadedPerfHooksHistogram.validatePercentile;
installHistogramPercentileMethods = loadedPerfHooksHistogram.installHistogramPercentileMethods;
fillPercentileMap = loadedPerfHooksHistogram.fillPercentileMap;
installHistogramPercentileAccessors = loadedPerfHooksHistogram.installHistogramPercentileAccessors;
installHistogramBaseMethods = loadedPerfHooksHistogram.installHistogramBaseMethods;
validateRecordValue = loadedPerfHooksHistogram.validateRecordValue;
installRecordableHistogramMethods = loadedPerfHooksHistogram.installRecordableHistogramMethods;
installIntervalHistogramMethods = loadedPerfHooksHistogram.installIntervalHistogramMethods;
createHistogram = loadedPerfHooksHistogram.createHistogram;
monitorEventLoopDelay = loadedPerfHooksHistogram.monitorEventLoopDelay;
installHistogram = loadedPerfHooksHistogram.installHistogram;
