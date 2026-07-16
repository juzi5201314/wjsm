// node:perf_hooks 共享内核：状态、brand 和 Node 风格校验。

function loadPerfHooksModule() {
let perfHost;
let performanceSingleton;
let rawStructuredClone;
let kInternal;
let kEntryBrand;
let kEntryName;
let kEntryType;
let kEntryStart;
let kEntryDuration;
let kEntryDetail;
let kUserTimingDetail;
let kResourceData;
let kObserverBrand;
let kObserverCallback;
let kObserverQueue;
let kObserverTypes;
let kObserverMode;
let kEntryListBrand;
let kEntryListEntries;
let kPerformanceBrand;
let kEventListeners;
let kResourceHandler;
let kHistogramBrand;
let kHistogramKind;
let kHistogramMap;
let kIntervalEnabled;
let kIntervalResolution;
let markTimings;
let markBuffer;
let measureBuffer;
let resourceBuffer;
let resourceSecondaryBuffer;
let resourceBufferLimit;
let resourceBufferFullPending;
let observers;
let pendingObservers;
let observerDispatchPending;
let supportedEntryTypes;
let nativeTypeBits;
let nativeObserverCounts;

// 分片 loader 通过 var 提前建立绑定，允许后续分片安全捕获前向引用。
var perfNow;
var makeError;
var invalidType;
var outOfRange;
var missingArgs;
var illegalConstructor;
var invalidThis;
var domException;
var implicitString;
var validateObject;
var validateNumber;
var validateInteger;
var requireOwnField;
var requireBrand;
var cloneDetail;
var stableEntrySort;
var defineEnumerable;
var defineFunctionMetadata;
var defineAccessorMetadata;
var internalTimelineForType;
var appendTimelineEntry;
var filterTimelineEntries;
var clearTimelineEntries;
var tryAppendResourceEntry;
var markResourceBufferFullPending;
var resourceSecondaryCount;
var resourceBufferAvailableCapacity;
var moveResourceSecondaryToPrimary;
var clearResourceSecondary;
var finishResourceBufferFull;
var setResourceBufferLimit;
var enqueuePerformanceEntry;
var incrementNativeType;
var decrementNativeType;
var updateNativeObserverState;
var isNativeTypeActive;
perfHost = globalThis.__wjsm_node_perf_hooks;
if (!perfHost) {
  throw new Error('wjsm internal perf_hooks host bridge is not installed');
}
performanceSingleton = globalThis.performance;
if (!performanceSingleton || typeof performanceSingleton.now !== 'function') {
  throw new Error('wjsm internal performance clock is not installed');
}
rawStructuredClone = perfHost.cloneDetail;
if (typeof rawStructuredClone !== 'function') {
  throw new Error('wjsm internal structured clone bridge is not installed');
}


kInternal = Symbol('perf_hooks.internal');
kEntryBrand = Symbol('perf_hooks.entry.brand');
kEntryName = Symbol('perf_hooks.entry.name');
kEntryType = Symbol('perf_hooks.entry.type');
kEntryStart = Symbol('perf_hooks.entry.start');
kEntryDuration = Symbol('perf_hooks.entry.duration');
kEntryDetail = Symbol('perf_hooks.entry.detail');
kUserTimingDetail = Symbol('perf_hooks.user-timing.detail');
kResourceData = Symbol('perf_hooks.resource.data');
kObserverBrand = Symbol('perf_hooks.observer.brand');
kObserverCallback = Symbol('perf_hooks.observer.callback');
kObserverQueue = Symbol('perf_hooks.observer.queue');
kObserverTypes = Symbol('perf_hooks.observer.types');
kObserverMode = Symbol('perf_hooks.observer.mode');
kEntryListBrand = Symbol('perf_hooks.entry-list.brand');
kEntryListEntries = Symbol('perf_hooks.entry-list.entries');
kPerformanceBrand = Symbol('perf_hooks.performance.brand');
kEventListeners = Symbol('perf_hooks.event.listeners');
kResourceHandler = Symbol('perf_hooks.resource.handler');
kHistogramBrand = Symbol('perf_hooks.histogram.brand');
kHistogramKind = Symbol('perf_hooks.histogram.kind');
kHistogramMap = Symbol('perf_hooks.histogram.map');
kIntervalEnabled = Symbol('perf_hooks.interval.enabled');
kIntervalResolution = Symbol('perf_hooks.interval.resolution');
markTimings = new Map();
markBuffer = [];
measureBuffer = [];
resourceBuffer = [];
resourceSecondaryBuffer = [];
resourceBufferLimit = 250;
resourceBufferFullPending = false;
observers = new Set();
pendingObservers = new Set();
observerDispatchPending = false;
supportedEntryTypes = Object.freeze([
  'dns', 'function', 'gc', 'http', 'http2',
  'mark', 'measure', 'net', 'resource',
]);
nativeTypeBits = Object.freeze({
  dns: 1,
  function: 2,
  gc: 4,
  http: 8,
  http2: 16,
  net: 32,
  resource: 64,
});
nativeObserverCounts = {
  dns: 0,
  function: 0,
  gc: 0,
  http: 0,
  http2: 0,
  net: 0,
  resource: 0,
};





function loadPerfHooksInternal(perfHost) {
let dnsObserverCount = 0;
let functionObserverCount = 0;
let gcObserverCount = 0;
let httpObserverCount = 0;
let http2ObserverCount = 0;


let netObserverCount = 0;


function perfNow() {
  return globalThis.performance.now();
}
function makeError(Ctor, code, message) {
  const error = new Ctor(message);
  if (code !== undefined) error.code = code;
  return error;
}

function invalidType(name, expected, value) {
  const actual = value === null ? 'null' : typeof value;
  return makeError(
    TypeError,
    'ERR_INVALID_ARG_TYPE',
    'The "' + name + '" argument must be of type ' + expected + '. Received ' + actual
  );
}

function outOfRange(name, range, value) {
  return makeError(
    RangeError,
    'ERR_OUT_OF_RANGE',
    'The value of "' + name + '" is out of range. It must be ' + range + '. Received ' + value
  );
}

function missingArgs(names) {
  return makeError(
    TypeError,
    'ERR_MISSING_ARGS',
    'The ' + names + ' argument must be specified'
  );
}

function illegalConstructor() {
  return makeError(TypeError, 'ERR_ILLEGAL_CONSTRUCTOR', 'Illegal constructor');
}

function invalidThis(name) {
  return makeError(TypeError, 'ERR_INVALID_THIS', 'Value of "this" must be of type ' + name);
}

function domException(message, name) {
  if (typeof globalThis.DOMException === 'function') {
    return new globalThis.DOMException(message, name);
  }
  const error = new Error(message);
  error.name = name;
  if (name === 'SyntaxError') error.code = 12;
  else if (name === 'InvalidModificationError') error.code = 13;
  return error;
}

function implicitString(value) {
  return '' + value;
}

function validateObject(value, name) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw invalidType(name, 'object', value);
  }
}

function validateNumber(value, name) {
  if (typeof value !== 'number') throw invalidType(name, 'number', value);
}

function validateInteger(value, name, minimum, maximum) {
  if (typeof value !== 'number') throw invalidType(name, 'number', value);
  if (!Number.isInteger(value)) throw outOfRange(name, 'an integer', value);
  if (value < minimum || (maximum !== undefined && value > maximum)) {
    const range = maximum === undefined ? '>= ' + minimum : '>= ' + minimum + ' && <= ' + maximum;
    throw outOfRange(name, range, value);
  }
}

function requireOwnField(value, field, name) {
  if (
    value === null ||
    value === undefined ||
    !Object.hasOwn(value, field)
  ) {
    throw invalidThis(name);
  }
}

function requireBrand(value, brand, name) {
  requireOwnField(value, brand, name);
  if (value[brand] !== true) throw invalidThis(name);
}

function cloneDetail(value) {
  if (value === null || value === undefined) return null;
  return rawStructuredClone(value);
}

function stableEntrySort(left, right) {
  return left.startTime - right.startTime;
}

function defineEnumerable(target, names) {
  if (target === null || (typeof target !== 'object' && typeof target !== 'function')) return;
  for (let i = 0; i < names.length; i = i + 1) {
    const name = names[i];
    const descriptor = Object.getOwnPropertyDescriptor(target, name);
    if (descriptor) {
      descriptor.enumerable = true;
      Object.defineProperty(target, name, descriptor);
    }
  }
}

function defineFunctionMetadata(callable, name, length) {
  if (typeof callable !== 'function') return callable;
  Object.defineProperties(callable, {
    name: {
      configurable: true,
      value: name,
    },
    length: {
      configurable: true,
      value: length,
    },
  });
  return callable;
}

function sealConstructor(Ctor) {
  Object.defineProperty(Ctor, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(Ctor.prototype, 'constructor', {
  value: Ctor,
  writable: true,
  enumerable: false,
  configurable: true,
});
  Object.defineProperty(Ctor.prototype, 'constructor', {
    value: Ctor,
    writable: true,
    enumerable: false,
    configurable: true,
  });
  return Ctor;
}

function defineAccessorMetadata(target, property, getterName, setterName) {
  if (target === null || (typeof target !== 'object' && typeof target !== 'function')) return;
  const descriptor = Object.getOwnPropertyDescriptor(target, property);
  if (descriptor === undefined) return;
  if (getterName !== undefined) {
    defineFunctionMetadata(descriptor.get, getterName, 0);
  }
  if (setterName !== undefined) {
    defineFunctionMetadata(descriptor.set, setterName, 1);
  }
}

function internalTimelineForType(type) {
  if (type === 'mark') return markBuffer;
  if (type === 'measure') return measureBuffer;
  if (type === 'resource') return resourceBuffer;
  return undefined;
}

function appendTimelineEntry(type, entry) {
  const buffer = internalTimelineForType(type);
  if (buffer !== undefined) buffer.push(entry);
}

function filterTimelineEntries(requestedName, requestedType) {
  let entries = [];
  const typedBuffer = internalTimelineForType(requestedType);
  if (requestedType !== undefined) {
    if (typedBuffer !== undefined) entries = typedBuffer.slice();
  } else {
    entries = markBuffer.concat(measureBuffer, resourceBuffer);
  }
  if (requestedName !== undefined) {
    const filtered = [];
    for (let index = 0; index < entries.length; index = index + 1) {
      if (entries[index].name === requestedName) filtered.push(entries[index]);
    }
    entries = filtered;
  }
  return entries.sort(stableEntrySort);
}

function clearTimelineEntries(requestedType, requestedName) {
  const buffer = internalTimelineForType(requestedType);
  if (buffer === undefined) return;
  if (requestedName === undefined) {
    buffer.splice(0, buffer.length);
    return;
  }
  for (let index = buffer.length - 1; index >= 0; index = index - 1) {
    if (buffer[index].name === requestedName) buffer.splice(index, 1);
  }
}

function tryAppendResourceEntry(entry) {
  if (resourceBuffer.length >= resourceBufferLimit || resourceBufferFullPending) {
    return false;
  }
  resourceBuffer.push(entry);
  return true;
}

function markResourceBufferFullPending(entry) {
  const schedule = !resourceBufferFullPending;
  resourceBufferFullPending = true;
  resourceSecondaryBuffer.push(entry);
  return schedule;
}

function resourceSecondaryCount() {
  return resourceSecondaryBuffer.length;
}

function resourceBufferAvailableCapacity() {
  return Math.max(resourceBufferLimit - resourceBuffer.length, 0);
}

function moveResourceSecondaryToPrimary(count) {
  for (let index = 0; index < count; index = index + 1) {
    resourceBuffer.push(resourceSecondaryBuffer[index]);
  }
  resourceSecondaryBuffer.splice(0, count);
}

function clearResourceSecondary() {
  resourceSecondaryBuffer.splice(0, resourceSecondaryBuffer.length);
}

function finishResourceBufferFull() {
  resourceBufferFullPending = false;
}

function setResourceBufferLimit(limit) {
  resourceBufferLimit = limit;
}



function enqueuePerformanceEntry(entry) {
  if (enqueueEntry !== undefined) enqueueEntry(entry);
}



function incrementNativeType(type) {
  if (type === 'dns') dnsObserverCount = dnsObserverCount + 1;
  else if (type === 'function') functionObserverCount = functionObserverCount + 1;
  else if (type === 'gc') gcObserverCount = gcObserverCount + 1;
  else if (type === 'http') httpObserverCount = httpObserverCount + 1;
  else if (type === 'http2') http2ObserverCount = http2ObserverCount + 1;
  else if (type === 'net') netObserverCount = netObserverCount + 1;
}

function decrementNativeType(type) {
  if (type === 'dns' && dnsObserverCount > 0) dnsObserverCount = dnsObserverCount - 1;
  else if (type === 'function' && functionObserverCount > 0) {
    functionObserverCount = functionObserverCount - 1;
  } else if (type === 'gc' && gcObserverCount > 0) gcObserverCount = gcObserverCount - 1;
  else if (type === 'http' && httpObserverCount > 0) httpObserverCount = httpObserverCount - 1;
  else if (type === 'http2' && http2ObserverCount > 0) {
    http2ObserverCount = http2ObserverCount - 1;
  } else if (type === 'net' && netObserverCount > 0) netObserverCount = netObserverCount - 1;
}

function nativeMask() {
  let mask = 64;
  if (dnsObserverCount > 0) mask = mask | 1;
  if (functionObserverCount > 0) mask = mask | 2;
  if (gcObserverCount > 0) mask = mask | 4;
  if (httpObserverCount > 0) mask = mask | 8;
  if (http2ObserverCount > 0) mask = mask | 16;
  if (netObserverCount > 0) mask = mask | 32;
  return mask;
}


function updateNativeObserverState() {
  perfHost.setObserverState(nativeMask());
}


function isNativeTypeActive(type) {
  if (type === 'resource') return true;
  if (type === 'dns') return dnsObserverCount > 0;
  if (type === 'function') return functionObserverCount > 0;
  if (type === 'gc') return gcObserverCount > 0;
  if (type === 'http') return httpObserverCount > 0;
  if (type === 'http2') return http2ObserverCount > 0;
  if (type === 'net') return netObserverCount > 0;
  return false;
}


  return {
    perfNow: perfNow,
    makeError: makeError,
    invalidType: invalidType,
    outOfRange: outOfRange,
    missingArgs: missingArgs,
    illegalConstructor: illegalConstructor,
    invalidThis: invalidThis,
    domException: domException,
    implicitString: implicitString,
    validateObject: validateObject,
    validateNumber: validateNumber,
    validateInteger: validateInteger,
    requireOwnField: requireOwnField,
    requireBrand: requireBrand,
    cloneDetail: cloneDetail,
    stableEntrySort: stableEntrySort,
    defineEnumerable: defineEnumerable,
    defineFunctionMetadata: defineFunctionMetadata,
    defineAccessorMetadata: defineAccessorMetadata,
    internalTimelineForType: internalTimelineForType,
    appendTimelineEntry: appendTimelineEntry,
    filterTimelineEntries: filterTimelineEntries,
    clearTimelineEntries: clearTimelineEntries,
    tryAppendResourceEntry: tryAppendResourceEntry,
    markResourceBufferFullPending: markResourceBufferFullPending,
    resourceSecondaryCount: resourceSecondaryCount,
    resourceBufferAvailableCapacity: resourceBufferAvailableCapacity,
    moveResourceSecondaryToPrimary: moveResourceSecondaryToPrimary,
    clearResourceSecondary: clearResourceSecondary,
    finishResourceBufferFull: finishResourceBufferFull,
    setResourceBufferLimit: setResourceBufferLimit,
    enqueuePerformanceEntry: enqueuePerformanceEntry,
    incrementNativeType: incrementNativeType,
    decrementNativeType: decrementNativeType,
    isNativeTypeActive: isNativeTypeActive,
    updateNativeObserverState: updateNativeObserverState,
  };
}

let loadedPerfHooksInternal = loadPerfHooksInternal(perfHost);
perfNow = loadedPerfHooksInternal.perfNow;
makeError = loadedPerfHooksInternal.makeError;
invalidType = loadedPerfHooksInternal.invalidType;
outOfRange = loadedPerfHooksInternal.outOfRange;
missingArgs = loadedPerfHooksInternal.missingArgs;
illegalConstructor = loadedPerfHooksInternal.illegalConstructor;
invalidThis = loadedPerfHooksInternal.invalidThis;
domException = loadedPerfHooksInternal.domException;
implicitString = loadedPerfHooksInternal.implicitString;
validateObject = loadedPerfHooksInternal.validateObject;
validateNumber = loadedPerfHooksInternal.validateNumber;
validateInteger = loadedPerfHooksInternal.validateInteger;
requireOwnField = loadedPerfHooksInternal.requireOwnField;
requireBrand = loadedPerfHooksInternal.requireBrand;
cloneDetail = loadedPerfHooksInternal.cloneDetail;
stableEntrySort = loadedPerfHooksInternal.stableEntrySort;
defineEnumerable = loadedPerfHooksInternal.defineEnumerable;
defineFunctionMetadata = loadedPerfHooksInternal.defineFunctionMetadata;
defineAccessorMetadata = loadedPerfHooksInternal.defineAccessorMetadata;
internalTimelineForType = loadedPerfHooksInternal.internalTimelineForType;
appendTimelineEntry = loadedPerfHooksInternal.appendTimelineEntry;
filterTimelineEntries = loadedPerfHooksInternal.filterTimelineEntries;
clearTimelineEntries = loadedPerfHooksInternal.clearTimelineEntries;
tryAppendResourceEntry = loadedPerfHooksInternal.tryAppendResourceEntry;
markResourceBufferFullPending = loadedPerfHooksInternal.markResourceBufferFullPending;
resourceSecondaryCount = loadedPerfHooksInternal.resourceSecondaryCount;
resourceBufferAvailableCapacity = loadedPerfHooksInternal.resourceBufferAvailableCapacity;
moveResourceSecondaryToPrimary = loadedPerfHooksInternal.moveResourceSecondaryToPrimary;
clearResourceSecondary = loadedPerfHooksInternal.clearResourceSecondary;
finishResourceBufferFull = loadedPerfHooksInternal.finishResourceBufferFull;
setResourceBufferLimit = loadedPerfHooksInternal.setResourceBufferLimit;
enqueuePerformanceEntry = loadedPerfHooksInternal.enqueuePerformanceEntry;
incrementNativeType = loadedPerfHooksInternal.incrementNativeType;
decrementNativeType = loadedPerfHooksInternal.decrementNativeType;
isNativeTypeActive = loadedPerfHooksInternal.isNativeTypeActive;
updateNativeObserverState = loadedPerfHooksInternal.updateNativeObserverState;
