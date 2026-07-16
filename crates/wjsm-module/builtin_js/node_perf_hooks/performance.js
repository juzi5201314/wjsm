// Performance singleton、Node timing、ELU、timerify 和最小 EventTarget。

var EventTarget;
var eventTargetListeners;
var installEventTargetPrototype;
var installEventTargetGlobal;
var dispatchPerformanceEvent;
var Performance;
var installPerformanceInheritance;
var requirePerformance;
var clearMarkTimings;
var installPerformanceMethods;
var toUnsignedLong;
var eventLoopUtilization;
var processTimerifyComplete;
var timerify;
var PerformanceNodeTiming;
var installNodeTimingPrototype;
var createNodeTiming;
var installPerformanceSurface;
var installPerformance;

function loadPerfHooksPerformance() {
function EventTarget() {
  if (new.target === undefined) {
    throw new TypeError("Class constructor EventTarget cannot be invoked without 'new'");
  }
  Object.defineProperty(this, kEventListeners, { value: {}, writable: true });
}

function eventTargetListeners(target) {
  if (target === null || target === undefined || target[kEventListeners] === undefined) {
    throw invalidThis('EventTarget');
  }
  return target[kEventListeners];
}

function installEventTargetPrototype() {
EventTarget.prototype.addEventListener = function (type, callback, options) {
  if (callback === null || callback === undefined) return;
  if (typeof callback !== 'function' && typeof callback.handleEvent !== 'function') return;
  const name = implicitString(type);
  const listeners = eventTargetListeners(this);
  if (listeners[name] === undefined) listeners[name] = [];
  const list = listeners[name];
  for (let i = 0; i < list.length; i = i + 1) {
    if (list[i].callback === callback) return;
  }
  list.push({
    callback: callback,
    once: options === true || (options && options.once === true),
  });
};

EventTarget.prototype.removeEventListener = function (type, callback) {
  const name = implicitString(type);
  const list = eventTargetListeners(this)[name];
  if (list === undefined) return;
  for (let i = list.length - 1; i >= 0; i = i - 1) {
    if (list[i].callback === callback) list.splice(i, 1);
  }
};

EventTarget.prototype.dispatchEvent = function (event) {
  if (event === null || event === undefined || event.type === undefined) {
    throw invalidType('event', 'Event', event);
  }
  const name = implicitString(event.type);
  const list = eventTargetListeners(this)[name];
  if (list === undefined || list.length === 0) return true;
  try {
    if (event.target === undefined) event.target = this;
    event.currentTarget = this;
  } catch (error) {
    // host Event 对象可能含只读 target；监听器仍应被派发。
  }
  const snapshot = list.slice();
  for (let i = 0; i < snapshot.length; i = i + 1) {
    const item = snapshot[i];
    if (item.once) this.removeEventListener(name, item.callback);
    if (typeof item.callback === 'function') item.callback.call(this, event);
    else item.callback.handleEvent.call(item.callback, event);
  }
  return event.defaultPrevented !== true;
};
defineFunctionMetadata(EventTarget.prototype.addEventListener, 'addEventListener', 2);
defineFunctionMetadata(EventTarget.prototype.removeEventListener, 'removeEventListener', 2);
defineFunctionMetadata(EventTarget.prototype.dispatchEvent, 'dispatchEvent', 1);
Object.defineProperty(EventTarget.prototype, Symbol.toStringTag, {
  configurable: true,
  value: 'EventTarget',
});
Object.defineProperty(EventTarget, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(EventTarget.prototype, 'constructor', {
  value: EventTarget,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function installEventTargetGlobal() {
  if (globalThis.EventTarget !== undefined) return;
  Object.defineProperty(globalThis, 'EventTarget', {
    configurable: true,
    enumerable: false,
    writable: true,
    value: EventTarget,
  });
}

function dispatchPerformanceEvent(type) {
  const event = {
    type: type,
    bubbles: false,
    cancelable: false,
    composed: false,
    defaultPrevented: false,
  };
  performanceSingleton.dispatchEvent(event);
}

function Performance() {
  if (new.target === undefined) {
    throw new TypeError("Class constructor Performance cannot be invoked without 'new'");
  }
  throw illegalConstructor();
}

function installPerformanceInheritance() {
Performance.prototype = Object.create(EventTarget.prototype);
Object.defineProperty(Performance.prototype, 'constructor', {
  value: Performance,
  writable: true,
  configurable: true,
});
Object.defineProperty(Performance, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(Performance.prototype, 'constructor', {
  value: Performance,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function requirePerformance(value) {
  requireBrand(value, kPerformanceBrand, 'Performance');
}

function clearMarkTimings(name) {
  if (name === undefined) {
    markTimings.clear();
    return;
  }
  if (reservedMarkNames !== undefined && Object.hasOwn(reservedMarkNames, name)) {
    throw makeError(
      TypeError,
      'ERR_INVALID_ARG_VALUE',
      'The argument \'name\' is invalid. Received \'' + name + '\''
    );
  }
  markTimings.delete(name);
}

function installPerformanceMethods() {
Performance.prototype.clearMarks = function (name) {
  requirePerformance(this);
  if (name !== undefined) name = implicitString(name);
  clearMarkTimings(name);
  clearTimeline('mark', name);
};

Performance.prototype.clearMeasures = function (name) {
  requirePerformance(this);
  if (name !== undefined) name = implicitString(name);
  clearTimeline('measure', name);
};

Performance.prototype.clearResourceTimings = function (name) {
  requirePerformance(this);
  if (name !== undefined) name = implicitString(name);
  clearTimeline('resource', name);
};

Performance.prototype.getEntries = function () {
  requirePerformance(this);
  return filterTimeline(undefined, undefined);
};

Performance.prototype.getEntriesByName = function (requestedName, requestedType) {
  requirePerformance(this);
  if (arguments.length === 0) throw missingArgs('"name"');
  requestedName = implicitString(requestedName);
  if (requestedType !== undefined) requestedType = implicitString(requestedType);
  return filterTimeline(requestedName, requestedType);
};

Performance.prototype.getEntriesByType = function (type) {
  requirePerformance(this);
  if (arguments.length === 0) throw missingArgs('"type"');
  return filterTimeline(undefined, implicitString(type));
};

Performance.prototype.mark = function (name, options) {
  requirePerformance(this);
  if (arguments.length === 0) throw missingArgs('"name"');
  return createUserMark(name, options);
};

Performance.prototype.measure = function (name, startOrOptions, endMark) {
  requirePerformance(this);
  if (arguments.length === 0) throw missingArgs('"name"');
  if (arguments.length < 2) startOrOptions = {};
  return createUserMeasure(name, startOrOptions, endMark);
};

Performance.prototype.now = function () {
  requirePerformance(this);
  return perfNow();
};

function toUnsignedLong(value) {
  if (typeof value === 'bigint') throw new TypeError('Cannot convert a BigInt value to a number');
  let number = +value;
  if (!Number.isFinite(number) || number === 0) return 0;
  number = number < 0 ? Math.ceil(number) : Math.floor(number);
  number = number % 4294967296;
  if (number < 0) number = number + 4294967296;
  return number;
}

Performance.prototype.setResourceTimingBufferSize = function (maxSize) {
  requirePerformance(this);
  if (arguments.length === 0) throw missingArgs('"maxSize"');
  setResourceBufferLimit(toUnsignedLong(maxSize));
};

Performance.prototype.markResourceTiming = function (
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
  return markResourceTiming(
    timingInfo,
    requestedUrl,
    initiatorType,
    global,
    cacheMode,
    bodyInfo,
    responseStatus,
    deliveryType
  );
};
defineFunctionMetadata(Performance.prototype.clearMarks, 'clearMarks', 0);
defineFunctionMetadata(Performance.prototype.clearMeasures, 'clearMeasures', 0);
defineFunctionMetadata(
  Performance.prototype.clearResourceTimings,
  'clearResourceTimings',
  0
);
defineFunctionMetadata(Performance.prototype.getEntries, 'getEntries', 0);
defineFunctionMetadata(Performance.prototype.getEntriesByName, 'getEntriesByName', 1);
defineFunctionMetadata(Performance.prototype.getEntriesByType, 'getEntriesByType', 1);
defineFunctionMetadata(Performance.prototype.mark, 'mark', 1);
defineFunctionMetadata(Performance.prototype.measure, 'measure', 1);
defineFunctionMetadata(Performance.prototype.now, 'now', 0);
defineFunctionMetadata(
  Performance.prototype.setResourceTimingBufferSize,
  'setResourceTimingBufferSize',
  1
);
defineFunctionMetadata(
  Performance.prototype.markResourceTiming,
  'markResourceTiming',
  7
);
}

function eventLoopUtilization(util1, util2) {
  const current = perfHost.eventLoopUtilization();
  if (util2) {
    const idle = util1.idle - util2.idle;
    const active = util1.active - util2.active;
    return { idle: idle, active: active, utilization: active / (idle + active) };
  }
  if (util1) {
    const idle = current.idle - util1.idle;
    const active = current.active - util1.active;
    return { idle: idle, active: active, utilization: active / (idle + active) };
  }
  return current;
}

function processTimerifyComplete(name, startTime, args, histogram) {
  const duration = perfNow() - startTime;
  if (histogram !== undefined) histogram.record(Math.ceil(duration * 1000000));
  const entry = createNodeEntry(name, 'function', startTime, duration, args);
  for (let i = 0; i < args.length; i = i + 1) entry[i] = args[i];
  enqueueEntry(entry);
}

function timerify(fn, options) {
  if (typeof fn !== 'function') throw invalidType('fn', 'function', fn);
  if (options === undefined) options = {};
  validateObject(options, 'options');
  const histogram = options.histogram;
  if (
    histogram !== undefined &&
    (!isHistogram(histogram) || histogramKind(histogram) !== HISTOGRAM_RECORDABLE)
  ) {
    throw invalidType('options.histogram', 'RecordableHistogram', histogram);
  }

  function timerified() {
    const args = Array.prototype.slice.call(arguments);
    const constructorCall = new.target !== undefined;
    const startTime = perfNow();
    let result;
    try {
      result = constructorCall
        ? Reflect.construct(fn, args, fn)
        : Reflect.apply(fn, this, args);
    } catch (error) {
      // Node: 同步抛错不产生 function entry，也不写入 histogram。
      throw error;
    }
    if (
      !constructorCall &&
      result !== null &&
      result !== undefined &&
      typeof result.then === 'function'
    ) {
      return result.then(
        function (value) {
          processTimerifyComplete(fn.name, startTime, args, histogram);
          return value;
        },
        function (error) {
          processTimerifyComplete(fn.name, startTime, args, histogram);
          throw error;
        }
      );
    }
    processTimerifyComplete(fn.name, startTime, args, histogram);
    return result;
  }

  if (Object.getOwnPropertyDescriptor(timerified, 'name').configurable) {
    Object.defineProperties(timerified, {
      length: {
        configurable: false,
        enumerable: true,
        writable: false,
        value: fn.length,
      },
      name: {
        configurable: false,
        enumerable: true,
        writable: false,
        value: 'timerified ' + fn.name,
      },
    });
  }
  return timerified;
}

function PerformanceNodeTiming() {
  Object.defineProperties(this, {
    name: { enumerable: true, configurable: true, value: 'node' },
    entryType: { enumerable: true, configurable: true, value: 'node' },
    startTime: { enumerable: true, configurable: true, value: 0 },
    duration: { enumerable: true, configurable: true, get: perfNow },
    uvMetricsInfo: {
      enumerable: true,
      configurable: true,
      get: function () {
        const snapshot = perfHost.nodeTiming();
        return {
          loopCount: snapshot.loopCount,
          events: snapshot.events,
          eventsWaiting: snapshot.eventsWaiting,
        };
      },
    },
    nodeStart: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().nodeStart; },
    },
    v8Start: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().v8Start; },
    },
    environment: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().environment; },
    },
    loopStart: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().loopStart; },
    },
    loopExit: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().loopExit; },
    },
    bootstrapComplete: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().bootstrapComplete; },
    },
    idleTime: {
      enumerable: true,
      configurable: true,
      get: function () { return perfHost.nodeTiming().idleTime; },
    },
  });
  defineAccessorMetadata(this, 'duration', 'get duration');
  defineAccessorMetadata(this, 'uvMetricsInfo', 'get uvMetricsInfo');
  defineAccessorMetadata(this, 'nodeStart', 'get nodeStart');
  defineAccessorMetadata(this, 'v8Start', 'get v8Start');
  defineAccessorMetadata(this, 'environment', 'get environment');
  defineAccessorMetadata(this, 'loopStart', 'get loopStart');
  defineAccessorMetadata(this, 'loopExit', 'get loopExit');
  defineAccessorMetadata(this, 'bootstrapComplete', 'get bootstrapComplete');
  defineAccessorMetadata(this, 'idleTime', 'get idleTime');
}

function installNodeTimingPrototype() {
PerformanceNodeTiming.prototype = Object.create(PerformanceEntry.prototype);
Object.defineProperty(PerformanceNodeTiming.prototype, 'constructor', {
  value: PerformanceNodeTiming,
  writable: true,
  configurable: true,
});
Object.defineProperty(PerformanceNodeTiming.prototype, 'toJSON', {
    configurable: true,
    writable: true,
    value: function () {
      return {
        name: 'node',
        entryType: 'node',
        startTime: this.startTime,
        duration: this.duration,
        nodeStart: this.nodeStart,
        v8Start: this.v8Start,
        bootstrapComplete: this.bootstrapComplete,
        environment: this.environment,
        loopStart: this.loopStart,
        loopExit: this.loopExit,
        idleTime: this.idleTime,
      };
    },
  });
defineFunctionMetadata(PerformanceNodeTiming.prototype.toJSON, 'toJSON', 0);
Object.defineProperty(PerformanceNodeTiming, 'prototype', {
  writable: false,
  configurable: false,
});
Object.defineProperty(PerformanceNodeTiming.prototype, 'constructor', {
  value: PerformanceNodeTiming,
  writable: true,
  enumerable: false,
  configurable: true,
});
}

function createNodeTiming() {
  return new PerformanceNodeTiming();
}

let nodeTiming;

function installPerformanceSurface() {
Performance.prototype.toJSON = function () {
  requirePerformance(this);
  return {
    nodeTiming: this.nodeTiming,
    timeOrigin: this.timeOrigin,
    eventLoopUtilization: this.eventLoopUtilization(),
  };
};

defineEnumerable(Performance.prototype, [
  'clearMarks',
  'clearMeasures',
  'clearResourceTimings',
  'getEntries',
  'getEntriesByName',
  'getEntriesByType',
  'mark',
  'measure',
  'now',
  'setResourceTimingBufferSize',
  'toJSON',
]);
Object.defineProperties(Performance.prototype, {
  timeOrigin: {
    enumerable: true,
    configurable: true,
    get: function () {
      requirePerformance(this);
      return perfHost.timeOrigin();
    },
  },
  eventLoopUtilization: {
    configurable: true,
    writable: true,
    value: eventLoopUtilization,
  },
  nodeTiming: {
    configurable: true,
    writable: true,
    value: nodeTiming,
  },
  timerify: {
    configurable: true,
    writable: true,
    value: timerify,
  },
  markResourceTiming: {
    configurable: true,
    writable: true,
    value: Performance.prototype.markResourceTiming,
  },
  onresourcetimingbufferfull: {
    enumerable: true,
    configurable: true,
    get: function () {
      requireOwnField(this, kResourceHandler, 'EventTarget');
      return this[kResourceHandler] || null;
    },
    set: function (listener) {
      requireOwnField(this, kResourceHandler, 'EventTarget');
      const prior = this[kResourceHandler];
      if (prior) this.removeEventListener('resourcetimingbufferfull', prior);
      this[kResourceHandler] = typeof listener === 'function' ? listener : null;
      if (this[kResourceHandler]) {
        this.addEventListener('resourcetimingbufferfull', this[kResourceHandler]);
      }
    },
  },
  [Symbol.toStringTag]: { configurable: true, value: 'Performance' },
});
defineFunctionMetadata(Performance.prototype.toJSON, 'toJSON', 0);
defineAccessorMetadata(Performance.prototype, 'timeOrigin', 'get timeOrigin');
defineAccessorMetadata(
  Performance.prototype,
  'onresourcetimingbufferfull',
  'get onresourcetimingbufferfull',
  'set onresourcetimingbufferfull',
);

Object.setPrototypeOf(performanceSingleton, Performance.prototype);
Object.defineProperties(performanceSingleton, {
  [kPerformanceBrand]: { value: true },
  [kEventListeners]: { value: {}, writable: true },
  [kResourceHandler]: { value: null, writable: true },
});
}

function installPerformance() {
  defineFunctionMetadata(EventTarget, 'EventTarget', 0);
  defineFunctionMetadata(Performance, 'Performance', 0);
  defineFunctionMetadata(eventLoopUtilization, 'eventLoopUtilization', 2);
  defineFunctionMetadata(timerify, 'timerify', 1);
  defineFunctionMetadata(PerformanceNodeTiming, 'PerformanceNodeTiming', 0);
  installEventTargetPrototype();
  installEventTargetGlobal();
  installPerformanceInheritance();
  installPerformanceMethods();
  installNodeTimingPrototype();
  nodeTiming = createNodeTiming();
  installPerformanceSurface();
}

return {
  EventTarget: EventTarget,
  eventTargetListeners: eventTargetListeners,
  installEventTargetPrototype: installEventTargetPrototype,
  installEventTargetGlobal: installEventTargetGlobal,
  dispatchPerformanceEvent: dispatchPerformanceEvent,
  Performance: Performance,
  installPerformanceInheritance: installPerformanceInheritance,
  requirePerformance: requirePerformance,
  clearMarkTimings: clearMarkTimings,
  installPerformanceMethods: installPerformanceMethods,
  toUnsignedLong: toUnsignedLong,
  eventLoopUtilization: eventLoopUtilization,
  processTimerifyComplete: processTimerifyComplete,
  timerify: timerify,
  PerformanceNodeTiming: PerformanceNodeTiming,
  installNodeTimingPrototype: installNodeTimingPrototype,
  createNodeTiming: createNodeTiming,
  installPerformanceSurface: installPerformanceSurface,
  installPerformance: installPerformance,
};
}

const perfHooksPerformance = loadPerfHooksPerformance();
EventTarget = perfHooksPerformance.EventTarget;
eventTargetListeners = perfHooksPerformance.eventTargetListeners;
installEventTargetPrototype = perfHooksPerformance.installEventTargetPrototype;
installEventTargetGlobal = perfHooksPerformance.installEventTargetGlobal;
dispatchPerformanceEvent = perfHooksPerformance.dispatchPerformanceEvent;
Performance = perfHooksPerformance.Performance;
installPerformanceInheritance = perfHooksPerformance.installPerformanceInheritance;
requirePerformance = perfHooksPerformance.requirePerformance;
clearMarkTimings = perfHooksPerformance.clearMarkTimings;
installPerformanceMethods = perfHooksPerformance.installPerformanceMethods;
toUnsignedLong = perfHooksPerformance.toUnsignedLong;
eventLoopUtilization = perfHooksPerformance.eventLoopUtilization;
processTimerifyComplete = perfHooksPerformance.processTimerifyComplete;
timerify = perfHooksPerformance.timerify;
PerformanceNodeTiming = perfHooksPerformance.PerformanceNodeTiming;
installNodeTimingPrototype = perfHooksPerformance.installNodeTimingPrototype;
createNodeTiming = perfHooksPerformance.createNodeTiming;
installPerformanceSurface = perfHooksPerformance.installPerformanceSurface;
installPerformance = perfHooksPerformance.installPerformance;
