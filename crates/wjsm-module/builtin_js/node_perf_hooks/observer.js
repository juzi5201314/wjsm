// PerformanceObserver 队列、timeline 查询和 native producer 接线。

var PerformanceObserverEntryList;
var installEntryListPrototype;
var scheduleObserverDispatch;
var dispatchPendingObservers;
var queueObserverEntry;
var enqueueEntry;
var enqueueNativeEntry;
var replaceObserverTypes;
var addObserverType;
var requireObserver;
var PerformanceObserver;
var installObserverPrototype;
var timelineForType;
var filterTimeline;
var clearTimeline;
var installObserver;

function loadPerfHooksObserver(dependencies) {
const incrementNativeType = dependencies.incrementNativeType;
const decrementNativeType = dependencies.decrementNativeType;
const updateNativeObserverState = dependencies.updateNativeObserverState;
const createNodeEntry = dependencies.createNodeEntry;
const observers = dependencies.observers;
const pendingObservers = dependencies.pendingObservers;
const kObserverCallback = dependencies.kObserverCallback;
const kObserverQueue = dependencies.kObserverQueue;
const kObserverTypes = dependencies.kObserverTypes;
const kInternal = dependencies.kInternal;

function PerformanceObserverEntryList(token, entries) {
  if (new.target === undefined) {
    throw new TypeError(
      "Class constructor PerformanceObserverEntryList cannot be invoked without 'new'"
    );
  }
  if (token !== kInternal) throw illegalConstructor();
  Object.defineProperties(this, {
    [kEntryListBrand]: { value: true },
    [kEntryListEntries]: { value: entries.slice().sort(stableEntrySort), writable: true },
  });
}

function installEntryListPrototype() {
PerformanceObserverEntryList.prototype.getEntries = function () {
  requireBrand(this, kEntryListBrand, 'PerformanceObserverEntryList');
  return this[kEntryListEntries].slice();
};

PerformanceObserverEntryList.prototype.getEntriesByType = function (type) {
  requireBrand(this, kEntryListBrand, 'PerformanceObserverEntryList');
  if (arguments.length === 0) throw missingArgs('"type"');
  const normalized = implicitString(type);
  const entries = this[kEntryListEntries];
  const filtered = [];
  for (let index = 0; index < entries.length; index = index + 1) {
    if (entries[index].entryType === normalized) filtered.push(entries[index]);
  }
  return filtered;
};

PerformanceObserverEntryList.prototype.getEntriesByName = function (requestedName, requestedType) {
  requireBrand(this, kEntryListBrand, 'PerformanceObserverEntryList');
  if (arguments.length === 0) throw missingArgs('"name"');
  const normalizedName = implicitString(requestedName);
  const entries = this[kEntryListEntries];
  const filtered = [];
  for (let index = 0; index < entries.length; index = index + 1) {
    const entry = entries[index];
    if (entry.name !== normalizedName) continue;
    if (
      requestedType === undefined ||
      requestedType === null ||
      entry.entryType === requestedType
    ) {
      filtered.push(entry);
    }
  }
  return filtered;
};

defineEnumerable(PerformanceObserverEntryList.prototype, [
  'getEntries',
  'getEntriesByType',
  'getEntriesByName',
]);
Object.defineProperty(PerformanceObserverEntryList.prototype, Symbol.toStringTag, {
  configurable: true,
  value: 'PerformanceObserverEntryList',
});
defineFunctionMetadata(
  PerformanceObserverEntryList.prototype.getEntries,
  'getEntries',
  0
);
defineFunctionMetadata(
  PerformanceObserverEntryList.prototype.getEntriesByType,
  'getEntriesByType',
  1
);
defineFunctionMetadata(
  PerformanceObserverEntryList.prototype.getEntriesByName,
  'getEntriesByName',
  1
);
Object.defineProperty(PerformanceObserverEntryList, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceObserverEntryList.prototype, 'constructor', {
  value: PerformanceObserverEntryList,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function dispatchPendingObservers() {
  observerDispatchPending = false;
  const pending = [];
  for (const observer of pendingObservers) pending.push(observer);
  pendingObservers.clear();
  for (let i = 0; i < pending.length; i = i + 1) {
    const observer = pending[i];
    if (!observers.has(observer)) continue;
    const entries = observer[kObserverQueue];
    observer[kObserverQueue] = [];
    const list = new PerformanceObserverEntryList(kInternal, entries);
    observer[kObserverCallback](list, observer);
  }
}

function scheduleObserverDispatch() {
  if (observerDispatchPending) return;
  observerDispatchPending = true;
  setImmediate(dispatchPendingObservers);
}

function queueObserverEntry(observer, entry) {
  if (!observer[kObserverTypes].includes(entry.entryType)) return;
  observer[kObserverQueue].push(entry);
  pendingObservers.add(observer);
  scheduleObserverDispatch();
}

function enqueueEntry(entry) {
  requireBrand(entry, kEntryBrand, 'PerformanceEntry');
  for (const observer of observers) queueObserverEntry(observer, entry);
}



function enqueueNativeEntry(raw) {
  const entry = createNodeEntry(
    raw.name,
    raw.entryType,
    raw.startTime,
    raw.duration,
    raw.detail
  );
  for (const observer of observers) {
    if (!observer[kObserverTypes].includes(entry.entryType)) continue;
    observer[kObserverQueue].push(entry);
    pendingObservers.add(observer);
  }
}

function replaceObserverTypes(observer, types) {
  for (const type of observer[kObserverTypes]) decrementNativeType(type);
  observer[kObserverTypes].length = 0;
  for (let i = 0; i < types.length; i = i + 1) {
    const type = types[i];
    if (!supportedEntryTypes.includes(type) || observer[kObserverTypes].includes(type)) continue;
    observer[kObserverTypes].push(type);
    incrementNativeType(type);
  }
  updateNativeObserverState();
}

function addObserverType(observer, type) {
  if (!supportedEntryTypes.includes(type)) return false;
  if (!observer[kObserverTypes].includes(type)) {
    observer[kObserverTypes].push(type);
    incrementNativeType(type);
    updateNativeObserverState();
  }
  return true;
}

function requireObserver(value) {
  if (
    value === null ||
    value === undefined ||
    !Object.hasOwn(value, kObserverBrand) ||
    value[kObserverBrand] !== true
  ) {
    throw new TypeError(
      'Cannot read private member from an object whose class did not declare it'
    );
  }
}

function PerformanceObserver(callback) {
  if (new.target === undefined) {
    throw new TypeError("Class constructor PerformanceObserver cannot be invoked without 'new'");
  }
  if (typeof callback !== 'function') throw invalidType('callback', 'function', callback);
  Object.defineProperties(this, {
    [kObserverBrand]: { value: true },
    [kObserverCallback]: { value: callback },
    [kObserverQueue]: { value: [], writable: true },
    [kObserverTypes]: { value: [] },
    [kObserverMode]: { value: undefined, writable: true },
  });
}

function installObserverPrototype() {
PerformanceObserver.prototype.observe = function (options) {
  requireObserver(this);
  if (options === undefined) options = {};
  validateObject(options, 'options');
  const copiedOptions = Object.assign({}, options);
  const entryTypes = copiedOptions.entryTypes;
  const type = copiedOptions.type;
  const buffered = copiedOptions.buffered;
  if (entryTypes === undefined && type === undefined) {
    throw missingArgs('"options.entryTypes" or "options.type"');
  }
  if (entryTypes !== null && entryTypes !== undefined && type !== null && type !== undefined) {
    throw makeError(
      TypeError,
      'ERR_INVALID_ARG_VALUE',
      'options.entryTypes can not be set with options.type'
    );
  }

  let requestedMode = type !== undefined ? 'single' : 'multiple';
  if (this[kObserverMode] === undefined) this[kObserverMode] = requestedMode;
  else if (this[kObserverMode] !== requestedMode) {
    const message = requestedMode === 'single'
      ? 'PerformanceObserver can not change to single observation'
      : 'PerformanceObserver can not change to multiple observations';
    throw domException(message, 'InvalidModificationError');
  }

  if (requestedMode === 'multiple') {
    if (!Array.isArray(entryTypes)) {
      throw invalidType('options.entryTypes', 'string[]', entryTypes);
    }
    replaceObserverTypes(this, entryTypes);
  } else {
    if (!addObserverType(this, type)) {
      return;
    }
    if (buffered) {
      const replay = filterTimeline(undefined, type);
      for (let i = 0; i < replay.length; i = i + 1) {
        this[kObserverQueue].push(replay[i]);
      }
      pendingObservers.add(this);
      scheduleObserverDispatch();
    }
  }

  if (this[kObserverTypes].length > 0) observers.add(this);
  else this.disconnect();
};

PerformanceObserver.prototype.disconnect = function () {
  requireObserver(this);
  for (const type of this[kObserverTypes]) decrementNativeType(type);
  updateNativeObserverState();
  observers.delete(this);
  pendingObservers.delete(this);
  this[kObserverQueue] = [];
  this[kObserverTypes].length = 0;
  this[kObserverMode] = undefined;
};

PerformanceObserver.prototype.takeRecords = function () {
  requireObserver(this);
  const records = this[kObserverQueue];
  this[kObserverQueue] = [];
  return records;
};

const supportedEntryTypesGetter = function () { return supportedEntryTypes; };
defineFunctionMetadata(
  supportedEntryTypesGetter,
  'get supportedEntryTypes',
  0
);
Object.defineProperty(PerformanceObserver, 'supportedEntryTypes', {
  configurable: true,
  enumerable: false,
  get: supportedEntryTypesGetter,
});
defineEnumerable(PerformanceObserver.prototype, ['observe', 'disconnect', 'takeRecords']);
Object.defineProperty(PerformanceObserver.prototype, Symbol.toStringTag, {
  configurable: true,
  value: 'PerformanceObserver',
});
defineFunctionMetadata(PerformanceObserver.prototype.observe, 'observe', 0);
defineFunctionMetadata(PerformanceObserver.prototype.disconnect, 'disconnect', 0);
defineFunctionMetadata(PerformanceObserver.prototype.takeRecords, 'takeRecords', 0);
Object.defineProperty(PerformanceObserver, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceObserver.prototype, 'constructor', {
  value: PerformanceObserver,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function timelineForType(type) {
  const buffer = internalTimelineForType(type);
  return buffer === undefined ? [] : buffer;
}

function filterTimeline(requestedName, requestedType) {
  return filterTimelineEntries(requestedName, requestedType);
}

function clearTimeline(requestedType, requestedName) {
  clearTimelineEntries(requestedType, requestedName);
}

function installObserver() {
  installEntryListPrototype();
  installObserverPrototype();
  defineFunctionMetadata(
    PerformanceObserverEntryList,
    'PerformanceObserverEntryList',
    0
  );
  defineFunctionMetadata(PerformanceObserver, 'PerformanceObserver', 1);
}

  return {
    PerformanceObserverEntryList: PerformanceObserverEntryList,
    installEntryListPrototype: installEntryListPrototype,
    scheduleObserverDispatch: scheduleObserverDispatch,
    dispatchPendingObservers: dispatchPendingObservers,
    queueObserverEntry: queueObserverEntry,
    enqueueEntry: enqueueEntry,
    enqueueNativeEntry: enqueueNativeEntry,
    replaceObserverTypes: replaceObserverTypes,
    addObserverType: addObserverType,
    requireObserver: requireObserver,
    PerformanceObserver: PerformanceObserver,
    installObserverPrototype: installObserverPrototype,
    timelineForType: timelineForType,
    filterTimeline: filterTimeline,
    clearTimeline: clearTimeline,
    installObserver: installObserver,
  };
}

let loadedPerfHooksObserver = loadPerfHooksObserver({
  incrementNativeType: incrementNativeType,
  decrementNativeType: decrementNativeType,
  updateNativeObserverState: updateNativeObserverState,
  createNodeEntry: createNodeEntry,
  observers: observers,
  pendingObservers: pendingObservers,
  kObserverCallback: kObserverCallback,
  kObserverQueue: kObserverQueue,
  kObserverTypes: kObserverTypes,
  kInternal: kInternal,
});
PerformanceObserverEntryList = loadedPerfHooksObserver.PerformanceObserverEntryList;
installEntryListPrototype = loadedPerfHooksObserver.installEntryListPrototype;
scheduleObserverDispatch = loadedPerfHooksObserver.scheduleObserverDispatch;
dispatchPendingObservers = loadedPerfHooksObserver.dispatchPendingObservers;
queueObserverEntry = loadedPerfHooksObserver.queueObserverEntry;
enqueueEntry = loadedPerfHooksObserver.enqueueEntry;
enqueueNativeEntry = loadedPerfHooksObserver.enqueueNativeEntry;
Object.defineProperty(globalThis, '__wjsm_perf_enqueueNative', {
  configurable: true,
  enumerable: false,
  writable: true,
  value: loadedPerfHooksObserver.enqueueNativeEntry,
});
Object.defineProperty(globalThis, '__wjsm_perf_dispatchNative', {
  configurable: true,
  enumerable: false,
  writable: true,
  value: loadedPerfHooksObserver.dispatchPendingObservers,
});
replaceObserverTypes = loadedPerfHooksObserver.replaceObserverTypes;
addObserverType = loadedPerfHooksObserver.addObserverType;
requireObserver = loadedPerfHooksObserver.requireObserver;
PerformanceObserver = loadedPerfHooksObserver.PerformanceObserver;
installObserverPrototype = loadedPerfHooksObserver.installObserverPrototype;
timelineForType = loadedPerfHooksObserver.timelineForType;
filterTimeline = loadedPerfHooksObserver.filterTimeline;
clearTimeline = loadedPerfHooksObserver.clearTimeline;
installObserver = loadedPerfHooksObserver.installObserver;
