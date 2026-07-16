// Resource Timing 条目、全局 buffer 和 native entry 归一化。

var PerformanceResourceTiming;
var installResourceInheritance;
var resourceData;
var installResourceAccessors;
var installResourceMethods;
var createPerformanceResourceTiming;
var dispatchResourceBufferFull;
var bufferResourceEntry;
var markResourceTiming;
var emitNativeEntry;
var installResource;
var timingInfoFromRaw;
var emitRawNativeEntry;
var emitNativeEntryRaw;

function loadPerfHooksResource(dependencies) {
const requireOwnField = dependencies.requireOwnField;
const enqueueEntry = dependencies.enqueueEntry;
const createNodeEntry = dependencies.createNodeEntry;
function PerformanceResourceTiming() {
  if (new.target === undefined) {
    throw new TypeError(
      "Class constructor PerformanceResourceTiming cannot be invoked without 'new'"
    );
  }
  throw illegalConstructor();
}

function installResourceInheritance() {
PerformanceResourceTiming.prototype = Object.create(PerformanceEntry.prototype);
Object.defineProperty(PerformanceResourceTiming.prototype, 'constructor', {
  value: PerformanceResourceTiming,
  writable: true,
  configurable: true,
});
}

function resourceData(entry) {
  if (
    entry === null ||
    entry === undefined ||
    !Object.hasOwn(entry, kResourceData)
  ) {
    const error = new TypeError(
      'Value of "this" must be of type PerformanceResourceTiming'
    );
    error.code = 'ERR_INVALID_THIS';
    throw error;
  }
  return entry[kResourceData];
}

function installResourceAccessors() {
Object.defineProperties(PerformanceResourceTiming.prototype, {
  name: {
    configurable: true,
    get: function () {
      if (!Object.hasOwn(this, kResourceData)) {
        const error = new TypeError(
          'Value of "this" must be of type PerformanceResourceTiming'
        );
        error.code = 'ERR_INVALID_THIS';
        throw error;
      }
      return this[kResourceData].requestedUrl;
    },
  },
  startTime: {
    configurable: true,
    get: function () { return resourceData(this).timingInfo.startTime; },
  },
  duration: {
    configurable: true,
    get: function () {
      const timing = resourceData(this).timingInfo;
      return timing.endTime - timing.startTime;
    },
  },
  initiatorType: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).initiatorType; },
  },
  workerStart: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.finalServiceWorkerStartTime; },
  },
  redirectStart: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.redirectStartTime; },
  },
  redirectEnd: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.redirectEndTime; },
  },
  fetchStart: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.postRedirectStartTime; },
  },
  domainLookupStart: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.domainLookupStartTime;
    },
  },
  domainLookupEnd: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.domainLookupEndTime;
    },
  },
  connectStart: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.connectionStartTime;
    },
  },
  connectEnd: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.connectionEndTime;
    },
  },
  secureConnectionStart: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.secureConnectionStartTime;
    },
  },
  nextHopProtocol: {
    enumerable: true,
    configurable: true,
    get: function () {
      const connection = resourceData(this).timingInfo.finalConnectionTimingInfo;
      return connection === undefined || connection === null
        ? undefined
        : connection.ALPNNegotiatedProtocol;
    },
  },
  requestStart: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.finalNetworkRequestStartTime; },
  },
  responseStart: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.finalNetworkResponseStartTime; },
  },
  responseEnd: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.endTime; },
  },
  encodedBodySize: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.encodedBodySize; },
  },
  decodedBodySize: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).timingInfo.decodedBodySize; },
  },
  transferSize: {
    enumerable: true,
    configurable: true,
    get: function () {
      const data = resourceData(this);
      if (data.cacheMode === 'local') return 0;
      return data.timingInfo.encodedBodySize + 300;
    },
  },
  deliveryType: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).deliveryType; },
  },
  responseStatus: {
    enumerable: true,
    configurable: true,
    get: function () { return resourceData(this).responseStatus; },
  },
  [Symbol.toStringTag]: { configurable: true, value: 'PerformanceResourceTiming' },
});
const accessorNames = [
  'name',
  'startTime',
  'duration',
  'initiatorType',
  'workerStart',
  'redirectStart',
  'redirectEnd',
  'fetchStart',
  'domainLookupStart',
  'domainLookupEnd',
  'connectStart',
  'connectEnd',
  'secureConnectionStart',
  'nextHopProtocol',
  'requestStart',
  'responseStart',
  'responseEnd',
  'encodedBodySize',
  'decodedBodySize',
  'transferSize',
  'deliveryType',
  'responseStatus',
];
for (
  let accessorIndex = 0;
  accessorIndex < accessorNames.length;
  accessorIndex = accessorIndex + 1
) {
  const name = accessorNames[accessorIndex];
  defineAccessorMetadata(PerformanceResourceTiming.prototype, name, 'get ' + name);
}
Object.defineProperty(PerformanceResourceTiming, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceResourceTiming.prototype, 'constructor', {
  value: PerformanceResourceTiming,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function installResourceMethods() {
PerformanceResourceTiming.prototype.toJSON = function () {
  resourceData(this);
  return {
    name: this.name,
    entryType: this.entryType,
    startTime: this.startTime,
    duration: this.duration,
    initiatorType: this.initiatorType,
    nextHopProtocol: this.nextHopProtocol,
    workerStart: this.workerStart,
    redirectStart: this.redirectStart,
    redirectEnd: this.redirectEnd,
    fetchStart: this.fetchStart,
    domainLookupStart: this.domainLookupStart,
    domainLookupEnd: this.domainLookupEnd,
    connectStart: this.connectStart,
    connectEnd: this.connectEnd,
    secureConnectionStart: this.secureConnectionStart,
    requestStart: this.requestStart,
    responseStart: this.responseStart,
    responseEnd: this.responseEnd,
    transferSize: this.transferSize,
    encodedBodySize: this.encodedBodySize,
    decodedBodySize: this.decodedBodySize,
    deliveryType: this.deliveryType,
    responseStatus: this.responseStatus,
  };
};
defineEnumerable(PerformanceResourceTiming.prototype, ['toJSON']);
defineFunctionMetadata(PerformanceResourceTiming.prototype.toJSON, 'toJSON', 0);
}

function createPerformanceResourceTiming(
  requestedUrl,
  initiatorType,
  timingInfo,
  cacheMode,
  responseStatus,
  deliveryType
) {
  const entry = Object.create(PerformanceResourceTiming.prototype);
  initializeEntry(entry, requestedUrl, 'resource', timingInfo.startTime, 0);
  Object.defineProperty(entry, kResourceData, {
    value: {
      requestedUrl: requestedUrl,
      initiatorType: initiatorType,
      timingInfo: timingInfo,
      cacheMode: cacheMode === undefined ? '' : cacheMode,
      responseStatus: responseStatus,
      deliveryType: deliveryType === undefined ? '' : deliveryType,
    },
  });
  return entry;
}

function dispatchResourceBufferFull() {
  while (resourceSecondaryCount() > 0) {
    const before = resourceSecondaryCount();
    dispatchPerformanceEvent('resourcetimingbufferfull');
    const preserve = Math.min(
      resourceBufferAvailableCapacity(),
      resourceSecondaryCount()
    );
    moveResourceSecondaryToPrimary(preserve);
    if (resourceSecondaryCount() >= before) clearResourceSecondary();
  }
  finishResourceBufferFull();
}

function bufferResourceEntry(entry) {
  if (tryAppendResourceEntry(entry)) return;
  if (markResourceBufferFullPending(entry)) setImmediate(dispatchResourceBufferFull);
}

function markResourceTiming(
  timingInfo,
  requestedUrl,
  initiatorType,
  global,
  cacheMode,
  bodyInfo,
  responseStatus,
  deliveryType
) {
  if (deliveryType === undefined) deliveryType = '';
  if (cacheMode !== '' && cacheMode !== 'local') {
    const error = new Error("cache must be an empty string or 'local'");
    error.code = 'ERR_INTERNAL_ASSERTION';
    throw error;
  }
  const entry = createPerformanceResourceTiming(
    requestedUrl,
    initiatorType,
    timingInfo,
    cacheMode,
    responseStatus,
    deliveryType
  );
  enqueueEntry(entry);
  bufferResourceEntry(entry);
  return entry;
}


function timingInfoFromRaw(startTime, duration, detail) {
  detail = detail || {};
  if (detail.timingInfo !== undefined) return detail.timingInfo;
  startTime = startTime === undefined ? 0 : startTime;
  duration = duration === undefined ? 0 : duration;
  return {
    startTime: startTime,
    redirectStartTime: detail.redirectStartTime === undefined ? 0 : detail.redirectStartTime,
    redirectEndTime: detail.redirectEndTime === undefined ? 0 : detail.redirectEndTime,
    postRedirectStartTime: detail.fetchStart === undefined ? startTime : detail.fetchStart,
    finalServiceWorkerStartTime: detail.workerStart === undefined ? 0 : detail.workerStart,
    finalNetworkRequestStartTime: detail.requestStart === undefined
      ? startTime
      : detail.requestStart,
    finalNetworkResponseStartTime: detail.responseStart === undefined
      ? startTime
      : detail.responseStart,
    endTime: detail.responseEnd === undefined ? startTime + duration : detail.responseEnd,
    encodedBodySize: detail.encodedBodySize === undefined ? 0 : detail.encodedBodySize,
    decodedBodySize: detail.decodedBodySize === undefined ? 0 : detail.decodedBodySize,
    finalConnectionTimingInfo: detail.finalConnectionTimingInfo,
  };
}

function emitRawNativeEntry(name, entryType, startTime, duration, detail) {
  if (entryType === 'resource') {
    detail = detail || {};
    markResourceTiming(
      timingInfoFromRaw(startTime, duration, detail),
      name,
      detail.initiatorType || 'fetch',
      globalThis,
      detail.cacheMode || '',
      detail.bodyInfo,
      detail.responseStatus === undefined ? 0 : detail.responseStatus,
      detail.deliveryType || ''
    );
    return;
  }
  enqueueEntry(createNodeEntry(name, entryType, startTime, duration, detail));
}


function emitNativeEntry(name, entryType, startTime, duration, detail) {
  emitRawNativeEntry(name, entryType, startTime, duration, detail);
  return true;
}

function emitNativeEntryRaw(raw) {
  const name = raw.name;
  const entryType = raw.entryType;
  const startTime = raw.startTime;
  const duration = raw.duration;
  const detail = raw.detail;
  emitRawNativeEntry(name, entryType, startTime, duration, detail);
  return true;
}

function installResource() {
  installResourceInheritance();
  installResourceAccessors();
  installResourceMethods();
  defineFunctionMetadata(
    PerformanceResourceTiming,
    'PerformanceResourceTiming',
    0
  );
}

return {
  PerformanceResourceTiming: PerformanceResourceTiming,
  installResourceInheritance: installResourceInheritance,
  resourceData: resourceData,
  installResourceAccessors: installResourceAccessors,
  installResourceMethods: installResourceMethods,
  createPerformanceResourceTiming: createPerformanceResourceTiming,
  dispatchResourceBufferFull: dispatchResourceBufferFull,
  bufferResourceEntry: bufferResourceEntry,
  markResourceTiming: markResourceTiming,
  timingInfoFromRaw: timingInfoFromRaw,
  emitRawNativeEntry: emitRawNativeEntry,
  emitNativeEntry: emitNativeEntry,
  emitNativeEntryRaw: emitNativeEntryRaw,
  installResource: installResource,
};
}

const perfHooksResource = loadPerfHooksResource({
  requireOwnField: requireOwnField,
  enqueueEntry: enqueueEntry,
  createNodeEntry: createNodeEntry,
});
PerformanceResourceTiming = perfHooksResource.PerformanceResourceTiming;
installResourceInheritance = perfHooksResource.installResourceInheritance;
resourceData = perfHooksResource.resourceData;
installResourceAccessors = perfHooksResource.installResourceAccessors;
installResourceMethods = perfHooksResource.installResourceMethods;
createPerformanceResourceTiming = perfHooksResource.createPerformanceResourceTiming;
dispatchResourceBufferFull = perfHooksResource.dispatchResourceBufferFull;
bufferResourceEntry = perfHooksResource.bufferResourceEntry;
markResourceTiming = perfHooksResource.markResourceTiming;
timingInfoFromRaw = perfHooksResource.timingInfoFromRaw;
emitRawNativeEntry = perfHooksResource.emitRawNativeEntry;
emitNativeEntry = perfHooksResource.emitNativeEntry;
emitNativeEntryRaw = perfHooksResource.emitNativeEntryRaw;
installResource = perfHooksResource.installResource;
