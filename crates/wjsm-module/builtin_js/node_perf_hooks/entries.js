// Performance Timeline 条目类型及 User Timing 计算。

let reservedMarkNames;

var initializeEntry;
var PerformanceEntry;
var installPerformanceEntryPrototype;
var PerformanceNodeEntry;
var installPerformanceNodeEntryPrototype;
var createNodeEntry;
var validateMarkName;
var PerformanceMark;
var installPerformanceMarkPrototype;
var PerformanceMeasure;
var installPerformanceMeasurePrototype;
var createMeasureEntry;
var nodeTimingMark;
var getMarkTime;
var calculateMeasure;
var createUserMark;
var createUserMeasure;
var installEntries;

function loadPerfHooksEntries() {

function initializeEntry(target, name, entryType, startTime, duration) {
  Object.defineProperties(target, {
    [kEntryBrand]: { value: true },
    [kEntryName]: {
      value: name,
      writable: true,
      enumerable: true,
      configurable: true,
    },
    [kEntryType]: {
      value: entryType,
      writable: true,
      enumerable: true,
      configurable: true,
    },
    [kEntryStart]: {
      value: startTime,
      writable: true,
      enumerable: true,
      configurable: true,
    },
    [kEntryDuration]: {
      value: duration,
      writable: true,
      enumerable: true,
      configurable: true,
    },
  });
  return target;
}

function PerformanceEntry() {
  if (new.target === undefined) {
    throw new TypeError("Class constructor PerformanceEntry cannot be invoked without 'new'");
  }
  throw illegalConstructor();
}

function installPerformanceEntryPrototype() {
Object.defineProperties(PerformanceEntry.prototype, {
  name: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return this[kEntryName];
    },
  },
  entryType: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return this[kEntryType];
    },
  },
  startTime: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return this[kEntryStart];
    },
  },
  duration: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return this[kEntryDuration];
    },
  },
  toJSON: {
    enumerable: true,
    configurable: true,
    writable: true,
    value: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return {
        name: this.name,
        entryType: this.entryType,
        startTime: this.startTime,
        duration: this.duration,
      };
    },
  },
});
for (const name of ['name', 'entryType', 'startTime', 'duration']) {
  defineAccessorMetadata(PerformanceEntry.prototype, name, 'get ' + name);
}
defineFunctionMetadata(PerformanceEntry.prototype.toJSON, 'toJSON', 0);
Object.defineProperty(PerformanceEntry, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceEntry.prototype, 'constructor', {
  value: PerformanceEntry,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function PerformanceNodeEntry() {
  throw illegalConstructor();
}

function installPerformanceNodeEntryPrototype() {
PerformanceNodeEntry.prototype = Object.create(PerformanceEntry.prototype);
Object.defineProperty(PerformanceNodeEntry.prototype, 'constructor', {
  value: PerformanceNodeEntry,
  writable: true,
  configurable: true,
});
Object.defineProperties(PerformanceNodeEntry.prototype, {
  detail: {
    configurable: true,
    get: function () {
      requireOwnField(this, kEntryDetail, 'NodePerformanceEntry');
      return this[kEntryDetail];
    },
  },
  toJSON: {
    configurable: true,
    writable: true,
    value: function () {
      requireBrand(this, kEntryBrand, 'PerformanceEntry');
      return {
        name: this[kEntryName],
        entryType: this[kEntryType],
        startTime: this[kEntryStart],
        duration: this[kEntryDuration],
        detail: this[kEntryDetail],
      };
    },
  },
});
defineAccessorMetadata(PerformanceNodeEntry.prototype, 'detail', 'get detail');
defineFunctionMetadata(PerformanceNodeEntry.prototype.toJSON, 'toJSON', 0);
Object.defineProperty(PerformanceNodeEntry, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceNodeEntry.prototype, 'constructor', {
  value: PerformanceNodeEntry,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function createNodeEntry(name, entryType, startTime, duration, detail) {
  const entry = Object.create(PerformanceNodeEntry.prototype);
  initializeEntry(entry, name, entryType, startTime, duration);
  Object.defineProperty(entry, kEntryDetail, {
    value: detail,
    writable: true,
    enumerable: true,
    configurable: true,
  });
  return entry;
}


function validateMarkName(name) {
  const value = implicitString(name);
  // 普通对象 + Object.hasOwn：hasOwnProperty.call 在部分可调用值上仍不可用。
  if (reservedMarkNames !== undefined && Object.hasOwn(reservedMarkNames, value)) {
    throw makeError(
      TypeError,
      'ERR_INVALID_ARG_VALUE',
      'The argument \'name\' is invalid. Received \'' + value + '\''
    );
  }
  return value;
}

function PerformanceMark(name, options) {
  if (new.target === undefined) {
    throw new TypeError("Class constructor PerformanceMark cannot be invoked without 'new'");
  }
  if (arguments.length === 0) throw missingArgs('"name"');
  const normalizedName = validateMarkName(name);
  if (options !== undefined && options !== null) validateObject(options, 'options');
  const optionStartTime = options === undefined || options === null
    ? undefined
    : options.startTime;
  let startTime = optionStartTime === undefined || optionStartTime === null
    ? perfNow()
    : optionStartTime;
  validateNumber(startTime, 'startTime');
  if (startTime < 0) {
    throw makeError(
      TypeError,
      'ERR_PERFORMANCE_INVALID_TIMESTAMP',
      startTime + ' is not a valid timestamp'
    );
  }
  const detail = options === undefined || options === null ? null : cloneDetail(options.detail);
  markTimings.set(normalizedName, startTime);
  initializeEntry(this, normalizedName, 'mark', startTime, 0);
  Object.defineProperty(this, kUserTimingDetail, {
    value: detail,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

function installPerformanceMarkPrototype() {
PerformanceMark.prototype = Object.create(PerformanceEntry.prototype);
Object.defineProperty(PerformanceMark.prototype, 'constructor', {
  value: PerformanceMark,
  writable: true,
  configurable: true,
});
Object.defineProperties(PerformanceMark.prototype, {
  detail: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireOwnField(this, kUserTimingDetail, 'PerformanceMark');
      return this[kUserTimingDetail];
    },
  },
  toJSON: {
    configurable: true,
    writable: true,
    value: function () {
      return {
        name: this.name,
        entryType: this.entryType,
        startTime: this.startTime,
        duration: this.duration,
        detail: this[kUserTimingDetail],
      };
    },
  },
  [Symbol.toStringTag]: { configurable: true, value: 'PerformanceMark' },
});
defineAccessorMetadata(PerformanceMark.prototype, 'detail', 'get detail');
defineFunctionMetadata(PerformanceMark.prototype.toJSON, 'toJSON', 0);
Object.defineProperty(PerformanceMark, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceMark.prototype, 'constructor', {
  value: PerformanceMark,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function PerformanceMeasure() {
  if (new.target === undefined) {
    throw new TypeError("Class constructor PerformanceMeasure cannot be invoked without 'new'");
  }
  throw illegalConstructor();
}

function installPerformanceMeasurePrototype() {
PerformanceMeasure.prototype = Object.create(PerformanceEntry.prototype);
Object.defineProperty(PerformanceMeasure.prototype, 'constructor', {
  value: PerformanceMeasure,
  writable: true,
  configurable: true,
});
Object.defineProperties(PerformanceMeasure.prototype, {
  detail: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireOwnField(this, kUserTimingDetail, 'PerformanceMeasure');
      return this[kUserTimingDetail];
    },
  },
  toJSON: {
    configurable: true,
    writable: true,
    value: PerformanceMark.prototype.toJSON,
  },
  [Symbol.toStringTag]: { configurable: true, value: 'PerformanceMeasure' },
});
defineAccessorMetadata(PerformanceMeasure.prototype, 'detail', 'get detail');
Object.defineProperty(PerformanceMeasure, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceMeasure.prototype, 'constructor', {
  value: PerformanceMeasure,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function createMeasureEntry(name, startTime, duration, detail) {
  const entry = Object.create(PerformanceMeasure.prototype);
  initializeEntry(entry, name, 'measure', startTime, duration);
  Object.defineProperty(entry, kUserTimingDetail, {
    value: detail,
    writable: true,
    enumerable: true,
    configurable: true,
  });
  return entry;
}

function nodeTimingMark(name) {
  if (reservedMarkNames === undefined || !Object.hasOwn(reservedMarkNames, name)) {
    return undefined;
  }
  return perfHost.nodeTiming()[name];
}

function getMarkTime(value) {
  if (value === undefined) return undefined;
  if (typeof value === 'number') {
    if (value < 0) {
      throw makeError(
        TypeError,
        'ERR_PERFORMANCE_INVALID_TIMESTAMP',
        value + ' is not a valid timestamp'
      );
    }
    return value;
  }
  const name = implicitString(value);
  const milestone = nodeTimingMark(name);
  if (milestone !== undefined) return milestone;
  const timing = markTimings.get(name);
  if (timing === undefined) {
    throw domException('The "' + name + '" performance mark has not been set', 'SyntaxError');
  }
  return timing;
}

function calculateMeasure(startOrOptions, endMark) {
  if (startOrOptions === null || startOrOptions === undefined) startOrOptions = 0;
  let start;
  let end;
  let duration;
  let optionsValid = false;
  if (typeof startOrOptions === 'object') {
    start = startOrOptions.start;
    end = startOrOptions.end;
    duration = startOrOptions.duration;
    optionsValid = start !== undefined || end !== undefined;
  }
  if (optionsValid) {
    if (endMark !== undefined) {
      throw makeError(
        TypeError,
        'ERR_PERFORMANCE_MEASURE_INVALID_OPTIONS',
        'endMark must not be specified'
      );
    }
    if (start === undefined && end === undefined) {
      throw makeError(
        TypeError,
        'ERR_PERFORMANCE_MEASURE_INVALID_OPTIONS',
        'One of options.start or options.end is required'
      );
    }
    if (start !== undefined && end !== undefined && duration !== undefined) {
      throw makeError(
        TypeError,
        'ERR_PERFORMANCE_MEASURE_INVALID_OPTIONS',
        'Must not have options.start, options.end, and options.duration specified'
      );
    }
  }

  if (endMark !== undefined) end = getMarkTime(endMark);
  else if (optionsValid && end !== undefined) end = getMarkTime(end);
  else if (optionsValid && start !== undefined && duration !== undefined) {
    end = getMarkTime(start) + getMarkTime(duration);
  } else end = perfNow();

  if (typeof startOrOptions === 'string') start = getMarkTime(startOrOptions);
  else if (optionsValid && start !== undefined) start = getMarkTime(start);
  else if (optionsValid && duration !== undefined && end !== undefined) {
    start = end - getMarkTime(duration);
  } else start = 0;
  return { start: start, duration: end - start };
}

function createUserMark(name, options) {
  // 先校验再 new：`return new Ctor()` 在构造器 throw 时目前会落到 unreachable。
  validateMarkName(name);
  const entry = new PerformanceMark(name, options);
  enqueuePerformanceEntry(entry);
  appendTimelineEntry('mark', entry);
  return entry;
}

function createUserMeasure(name, startOrOptions, endMark) {
  if (typeof name !== 'string') throw invalidType('name', 'string', name);
  const timing = calculateMeasure(startOrOptions, endMark);
  const rawDetail = startOrOptions === null || startOrOptions === undefined
    ? undefined
    : startOrOptions.detail;
  const detail = cloneDetail(rawDetail);
  const entry = createMeasureEntry(name, timing.start, timing.duration, detail);
  enqueuePerformanceEntry(entry);
  appendTimelineEntry('measure', entry);
  return entry;
}

function installEntries() {
  reservedMarkNames = {
    nodeStart: true,
    v8Start: true,
    environment: true,
    loopStart: true,
    loopExit: true,
    bootstrapComplete: true,
  };
  installPerformanceEntryPrototype();
  installPerformanceNodeEntryPrototype();
  installPerformanceMarkPrototype();
  installPerformanceMeasurePrototype();
  defineFunctionMetadata(PerformanceEntry, 'PerformanceEntry', 0);
  defineFunctionMetadata(PerformanceNodeEntry, 'PerformanceNodeEntry', 0);
  defineFunctionMetadata(PerformanceMark, 'PerformanceMark', 1);
  defineFunctionMetadata(PerformanceMeasure, 'PerformanceMeasure', 0);
}

  return {
    initializeEntry: initializeEntry,
    PerformanceEntry: PerformanceEntry,
    installPerformanceEntryPrototype: installPerformanceEntryPrototype,
    PerformanceNodeEntry: PerformanceNodeEntry,
    installPerformanceNodeEntryPrototype: installPerformanceNodeEntryPrototype,
    createNodeEntry: createNodeEntry,
    validateMarkName: validateMarkName,
    PerformanceMark: PerformanceMark,
    installPerformanceMarkPrototype: installPerformanceMarkPrototype,
    PerformanceMeasure: PerformanceMeasure,
    installPerformanceMeasurePrototype: installPerformanceMeasurePrototype,
    createMeasureEntry: createMeasureEntry,
    nodeTimingMark: nodeTimingMark,
    getMarkTime: getMarkTime,
    calculateMeasure: calculateMeasure,
    createUserMark: createUserMark,
    createUserMeasure: createUserMeasure,
    installEntries: installEntries,
  };
}

let loadedPerfHooksEntries = loadPerfHooksEntries();
initializeEntry = loadedPerfHooksEntries.initializeEntry;
PerformanceEntry = loadedPerfHooksEntries.PerformanceEntry;
installPerformanceEntryPrototype = loadedPerfHooksEntries.installPerformanceEntryPrototype;
PerformanceNodeEntry = loadedPerfHooksEntries.PerformanceNodeEntry;
installPerformanceNodeEntryPrototype = loadedPerfHooksEntries.installPerformanceNodeEntryPrototype;
createNodeEntry = loadedPerfHooksEntries.createNodeEntry;
validateMarkName = loadedPerfHooksEntries.validateMarkName;
PerformanceMark = loadedPerfHooksEntries.PerformanceMark;
installPerformanceMarkPrototype = loadedPerfHooksEntries.installPerformanceMarkPrototype;
PerformanceMeasure = loadedPerfHooksEntries.PerformanceMeasure;
installPerformanceMeasurePrototype = loadedPerfHooksEntries.installPerformanceMeasurePrototype;
createMeasureEntry = loadedPerfHooksEntries.createMeasureEntry;
nodeTimingMark = loadedPerfHooksEntries.nodeTimingMark;
getMarkTime = loadedPerfHooksEntries.getMarkTime;
calculateMeasure = loadedPerfHooksEntries.calculateMeasure;
createUserMark = loadedPerfHooksEntries.createUserMark;
createUserMeasure = loadedPerfHooksEntries.createUserMeasure;
installEntries = loadedPerfHooksEntries.installEntries;
