const hooks = require("node:perf_hooks");
const {
  Performance,
  PerformanceEntry,
  PerformanceMark,
  PerformanceMeasure,
  PerformanceObserver,
  PerformanceObserverEntryList,
  PerformanceResourceTiming,
  performance,
  timerify,
} = hooks;

function descriptor(target, name) {
  return Object.getOwnPropertyDescriptor(target, name);
}

function rejectsWithCode(action, code) {
  try {
    action();
    return false;
  } catch (error) {
    return error.name === "TypeError" && error.code === code;
  }
}

function rejectsInvalidThis(action) {
  return rejectsWithCode(action, "ERR_INVALID_THIS");
}

const constructors = [
  Performance,
  PerformanceEntry,
  PerformanceMark,
  PerformanceMeasure,
  PerformanceObserver,
  PerformanceObserverEntryList,
  PerformanceResourceTiming,
];
const constructorNames = [
  "Performance",
  "PerformanceEntry",
  "PerformanceMark",
  "PerformanceMeasure",
  "PerformanceObserver",
  "PerformanceObserverEntryList",
  "PerformanceResourceTiming",
];
console.log(
  constructors.every((Ctor, index) => {
    const prototype = descriptor(Ctor, "prototype");
    const constructor = descriptor(Ctor.prototype, "constructor");
    return (
      Ctor.name === constructorNames[index] &&
      prototype.writable === false &&
      prototype.configurable === false &&
      constructor.value === Ctor &&
      constructor.writable === true &&
      constructor.enumerable === false &&
      constructor.configurable === true
    );
  }) &&
    Performance.length === 0 &&
    PerformanceEntry.length === 0 &&
    PerformanceMark.length === 1 &&
    PerformanceMeasure.length === 0 &&
    PerformanceObserver.length === 1 &&
    PerformanceObserverEntryList.length === 0 &&
    PerformanceResourceTiming.length === 0 &&
    timerify.length === 1 &&
    timerify.name === "timerify" &&
    Performance.prototype.clearMarks.length === 0 &&
    Performance.prototype.getEntriesByName.length === 1 &&
    Performance.prototype.mark.length === 1 &&
    Performance.prototype.measure.length === 1 &&
    Performance.prototype.markResourceTiming.length === 7 &&
    PerformanceObserver.prototype.observe.length === 0 &&
    PerformanceObserverEntryList.prototype.getEntriesByName.length === 1
);

console.log(
  descriptor(PerformanceEntry.prototype, "toJSON").enumerable === true &&
    descriptor(PerformanceMark.prototype, "detail").enumerable === true &&
    descriptor(PerformanceMark.prototype, "toJSON").enumerable === false &&
    descriptor(PerformanceMeasure.prototype, "toJSON").enumerable === false &&
    descriptor(PerformanceResourceTiming.prototype, "name").enumerable === false &&
    descriptor(PerformanceResourceTiming.prototype, "duration").enumerable === false &&
    descriptor(PerformanceResourceTiming.prototype, "initiatorType").enumerable === true &&
    descriptor(PerformanceResourceTiming.prototype, "toJSON").enumerable === true &&
    descriptor(Performance.prototype, "timeOrigin").enumerable === true &&
    descriptor(Performance.prototype, "timerify").enumerable === false &&
    Performance.prototype.clearMarks.name === "clearMarks" &&
    Performance.prototype.getEntriesByName.name === "getEntriesByName" &&
    Performance.prototype.mark.name === "mark" &&
    Performance.prototype.measure.name === "measure" &&
    Performance.prototype.markResourceTiming.name === "markResourceTiming" &&
    PerformanceObserver.prototype.observe.name === "observe" &&
    PerformanceObserverEntryList.prototype.getEntriesByName.name ===
      "getEntriesByName" &&
    descriptor(PerformanceEntry.prototype, "name").get.name === "get name" &&
    descriptor(PerformanceMark.prototype, "detail").get.name === "get detail" &&
    descriptor(PerformanceResourceTiming.prototype, "initiatorType").get.name ===
      "get initiatorType" &&
    Object.isFrozen(hooks.constants) === false &&
    descriptor(hooks.constants, "NODE_PERFORMANCE_GC_MINOR").enumerable === true &&
    descriptor(hooks.constants, "NODE_PERFORMANCE_GC_MINOR").writable === false &&
    descriptor(hooks.constants, "NODE_PERFORMANCE_GC_MINOR").configurable === false &&
    typeof globalThis.EventTarget === "function" &&
    performance instanceof globalThis.EventTarget &&
    Object.getPrototypeOf(Performance.prototype) === globalThis.EventTarget.prototype &&
    Object.getPrototypeOf(Performance.prototype).constructor === globalThis.EventTarget &&
    descriptor(globalThis, "EventTarget").enumerable === false &&
    descriptor(globalThis, "EventTarget").writable === true &&
    descriptor(globalThis, "EventTarget").configurable === true
);

console.log(
  constructors.every((Ctor) => rejectsWithCode(() => Ctor(), undefined)) &&
    rejectsWithCode(() => new Performance(), "ERR_ILLEGAL_CONSTRUCTOR") &&
    rejectsWithCode(() => new PerformanceEntry(), "ERR_ILLEGAL_CONSTRUCTOR") &&
    rejectsWithCode(() => new PerformanceMeasure(), "ERR_ILLEGAL_CONSTRUCTOR") &&
    rejectsWithCode(
      () => new PerformanceObserverEntryList(),
      "ERR_ILLEGAL_CONSTRUCTOR"
    ) &&
    rejectsWithCode(
      () => new PerformanceResourceTiming(),
      "ERR_ILLEGAL_CONSTRUCTOR"
    ) &&
    rejectsWithCode(() => new PerformanceMark(), "ERR_MISSING_ARGS") &&
    rejectsWithCode(() => new PerformanceObserver(), "ERR_INVALID_ARG_TYPE")
);

performance.clearMarks();
performance.clearMeasures();
let startTimeReads = 0;
const sourceDetail = { nested: { value: 1 } };
const originalStructuredClone = globalThis.structuredClone;
globalThis.structuredClone = () => ({ tampered: true });
const exactMark = new PerformanceMark("exact-mark", {
  get startTime() {
    startTimeReads += 1;
    return 5;
  },
  detail: sourceDetail,
});
globalThis.structuredClone = originalStructuredClone;
sourceDetail.nested.value = 2;

const cyclicDetail = {};
cyclicDetail.self = cyclicDetail;
const cyclicMark = new PerformanceMark("cyclic-mark", { detail: cyclicDetail });
function functionMeasureOptions() {}
functionMeasureOptions.detail = { value: 9 };
const functionMeasure = performance.measure(
  "function-options-detail",
  functionMeasureOptions
);
functionMeasureOptions.detail.value = 10;

console.log(
  startTimeReads === 1 &&
    exactMark.startTime === 5 &&
    exactMark.detail.nested.value === 1 &&
    cyclicMark.detail !== cyclicDetail &&
    cyclicMark.detail.self === cyclicMark.detail &&
    functionMeasure.startTime === 0 &&
    functionMeasure.detail.value === 9 &&
    rejectsWithCode(() => new PerformanceMark("array", []), "ERR_INVALID_ARG_TYPE") &&
    rejectsWithCode(
      () => new PerformanceMark("function", function () {}),
      "ERR_INVALID_ARG_TYPE"
    ) &&
    rejectsWithCode(() => timerify(() => {}, []), "ERR_INVALID_ARG_TYPE") &&
    rejectsWithCode(
      () => timerify(() => {}, function () {}),
      "ERR_INVALID_ARG_TYPE"
    )
);

const markChild = Object.create(exactMark);
const measureChild = Object.create(functionMeasure);
const performanceChild = Object.create(performance);
console.log(
  rejectsInvalidThis(() => descriptor(PerformanceEntry.prototype, "name").get.call(markChild)) &&
    rejectsInvalidThis(() => descriptor(PerformanceMark.prototype, "detail").get.call(markChild)) &&
    rejectsInvalidThis(
      () => descriptor(PerformanceMeasure.prototype, "detail").get.call(measureChild)
    ) &&
    rejectsInvalidThis(() => Performance.prototype.now.call(performanceChild)) &&
    rejectsWithCode(
      () => PerformanceObserver.prototype.takeRecords.call(
        Object.create(new PerformanceObserver(() => {}))
      ),
      undefined
    ) &&
    rejectsInvalidThis(
      () => descriptor(Performance.prototype, "onresourcetimingbufferfull").get.call(
        performanceChild
      )
    )
);

let copiedOptionReads = 0;
const optionObserver = new PerformanceObserver(() => {});
const inheritedOptions = Object.create({ type: "mark" });
const nonEnumerableOptions = {};
Object.defineProperty(nonEnumerableOptions, "type", { value: "mark" });
const copiedOptions = { entryTypes: ["mark"] };
Object.defineProperty(copiedOptions, "unused", {
  enumerable: true,
  get() {
    copiedOptionReads += 1;
    return true;
  },
});
console.log(
  rejectsWithCode(
    () => optionObserver.observe(inheritedOptions),
    "ERR_MISSING_ARGS"
  ) &&
    rejectsWithCode(
      () => optionObserver.observe(nonEnumerableOptions),
      "ERR_MISSING_ARGS"
    ) &&
    rejectsWithCode(() => optionObserver.observe([]), "ERR_INVALID_ARG_TYPE")
);
optionObserver.observe(copiedOptions);
optionObserver.disconnect();
console.log(copiedOptionReads === 1);

function timing(startTime) {
  return {
    startTime,
    redirectStartTime: 0,
    redirectEndTime: 0,
    postRedirectStartTime: startTime,
    finalServiceWorkerStartTime: 0,
    finalNetworkRequestStartTime: startTime,
    finalNetworkResponseStartTime: startTime,
    endTime: startTime + 1,
    encodedBodySize: 0,
    decodedBodySize: 0,
    finalConnectionTimingInfo: null,
  };
}

const resource = performance.markResourceTiming(
  timing(2),
  "resource-exact",
  "fetch",
  globalThis,
  "",
  {},
  200,
  null
);
const resourceChild = Object.create(resource);
console.log(
  resource.deliveryType === null &&
    resource.domainLookupStart === undefined &&
    rejectsInvalidThis(
      () => descriptor(PerformanceResourceTiming.prototype, "name").get.call(resourceChild)
    ) &&
    rejectsInvalidThis(() => PerformanceResourceTiming.prototype.toJSON.call(resourceChild)) &&
    rejectsWithCode(
      () => performance.setResourceTimingBufferSize(1n),
      undefined
    )
);

const wrapped = timerify(function original(a, b) {
  return a + b;
});
const wrappedLength = descriptor(wrapped, "length");
const wrappedName = descriptor(wrapped, "name");
console.log(
  wrapped(1, 2) === 3 &&
    wrapped.length === 2 &&
    wrapped.name === "timerified original" &&
    wrappedLength.enumerable === true &&
    wrappedLength.configurable === false &&
    wrappedLength.writable === false &&
    wrappedName.enumerable === true &&
    wrappedName.configurable === false &&
    wrappedName.writable === false
);

const nodeTiming = performance.nodeTiming;
const nodeTimingPrototype = Object.getPrototypeOf(nodeTiming);
const performanceJSON = performance.toJSON();
console.log(
  nodeTiming instanceof PerformanceEntry &&
    nodeTimingPrototype !== PerformanceEntry.prototype &&
    Object.getPrototypeOf(nodeTimingPrototype) === PerformanceEntry.prototype &&
    nodeTimingPrototype.constructor.name === "PerformanceNodeTiming" &&
    descriptor(nodeTimingPrototype, "toJSON").enumerable === false &&
    Object.keys(nodeTiming.toJSON()).join(",") ===
      "name,entryType,startTime,duration,nodeStart,v8Start,bootstrapComplete,environment,loopStart,loopExit,idleTime" &&
    performanceJSON.nodeTiming === nodeTiming &&
    performanceJSON.timeOrigin === performance.timeOrigin &&
    typeof performanceJSON.eventLoopUtilization === "object" &&
    Number.isFinite(performance.timeOrigin) &&
    performance.timeOrigin > 0
);

let callbackThis;
let listObserver;
listObserver = new PerformanceObserver(function (list, self) {
  callbackThis = this;
  const type = {
    toString() {
      throw new Error("type must not be coerced");
    },
  };
  console.log(
    self === listObserver &&
      callbackThis === listObserver &&
      Object.prototype.toString.call(list) === "[object PerformanceObserverEntryList]" &&
      list.getEntriesByName("observer-mark", type).length === 0 &&
      rejectsInvalidThis(
        () => PerformanceObserverEntryList.prototype.getEntries.call(Object.create(list))
      )
  );
  listObserver.disconnect();
  performance.clearMarks();
  performance.clearMeasures();
  performance.clearResourceTimings();
});
listObserver.observe({ type: "mark" });
performance.mark("observer-mark");
