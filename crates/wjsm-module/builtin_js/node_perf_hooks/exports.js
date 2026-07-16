// Node v24.15 公共导出与全局 constructor 安装。

let constants;
let perfHooks;



function drainNativePerformanceEntries() {
  return globalThis.__wjsm_node_perf_hooks.drainNativeEntry();
}

function installPerfHooksExports() {
installEntries();
installObserver();
installHistogram();
installResource();
installPerformance();
const constantValues = {
  NODE_PERFORMANCE_GC_MINOR: 1,
  NODE_PERFORMANCE_GC_MAJOR: 4,
  NODE_PERFORMANCE_GC_INCREMENTAL: 8,
  NODE_PERFORMANCE_GC_WEAKCB: 16,
  NODE_PERFORMANCE_GC_FLAGS_NO: 0,
  NODE_PERFORMANCE_GC_FLAGS_CONSTRUCT_RETAINED: 2,
  NODE_PERFORMANCE_GC_FLAGS_FORCED: 4,
  NODE_PERFORMANCE_GC_FLAGS_SYNCHRONOUS_PHANTOM_PROCESSING: 8,
  NODE_PERFORMANCE_GC_FLAGS_ALL_AVAILABLE_GARBAGE: 16,
  NODE_PERFORMANCE_GC_FLAGS_ALL_EXTERNAL_MEMORY: 32,
  NODE_PERFORMANCE_GC_FLAGS_SCHEDULE_IDLE: 64,
};
const installedConstants = {};
const constantNames = Object.keys(constantValues);
for (
  let constantIndex = 0;
  constantIndex < constantNames.length;
  constantIndex = constantIndex + 1
) {
  const name = constantNames[constantIndex];
  Object.defineProperty(installedConstants, name, {
    enumerable: true,
    value: constantValues[name],
  });
}
constants = installedConstants;

const globalConstructors = {
  Performance: Performance,
  PerformanceEntry: PerformanceEntry,
  PerformanceMark: PerformanceMark,
  PerformanceMeasure: PerformanceMeasure,
  PerformanceObserver: PerformanceObserver,
  PerformanceObserverEntryList: PerformanceObserverEntryList,
  PerformanceResourceTiming: PerformanceResourceTiming,
};
const globalConstructorNames = Object.keys(globalConstructors);
for (let i = 0; i < globalConstructorNames.length; i = i + 1) {
  const name = globalConstructorNames[i];
  Object.defineProperty(globalThis, name, {
    configurable: true,
    enumerable: false,
    writable: true,
    value: globalConstructors[name],
  });
}

Object.defineProperty(emitNativeEntry, 'enabled', {
  configurable: false,
  enumerable: false,
  writable: false,
  value: isNativeTypeActive,
});
Object.defineProperty(globalThis, '__wjsm_perf_emitNative', {
  configurable: true,
  enumerable: false,
  writable: true,
  value: emitNativeEntry,
});
Object.defineProperty(globalThis, '__wjsm_perf_emitNativeRaw', {
  configurable: true,
  enumerable: false,
  writable: true,
  value: emitNativeEntryRaw,
});
perfHost.setObserverState(64, drainNativePerformanceEntries);



const installedPerfHooks = {
  Performance: Performance,
  PerformanceEntry: PerformanceEntry,
  PerformanceMark: PerformanceMark,
  PerformanceMeasure: PerformanceMeasure,
  PerformanceObserver: PerformanceObserver,
  PerformanceObserverEntryList: PerformanceObserverEntryList,
  PerformanceResourceTiming: PerformanceResourceTiming,
  monitorEventLoopDelay: monitorEventLoopDelay,
  eventLoopUtilization: eventLoopUtilization,
  timerify: timerify,
  createHistogram: createHistogram,
  performance: performanceSingleton,
};
Object.defineProperty(installedPerfHooks, 'constants', {
  configurable: false,
  enumerable: true,
  value: installedConstants,
});
perfHooks = installedPerfHooks;
}

installPerfHooksExports();

return {
  Performance: Performance,
  PerformanceEntry: PerformanceEntry,
  PerformanceMark: PerformanceMark,
  PerformanceMeasure: PerformanceMeasure,
  PerformanceObserver: PerformanceObserver,
  PerformanceObserverEntryList: PerformanceObserverEntryList,
  PerformanceResourceTiming: PerformanceResourceTiming,
  monitorEventLoopDelay: monitorEventLoopDelay,
  eventLoopUtilization: eventLoopUtilization,
  timerify: timerify,
  createHistogram: createHistogram,
  performance: performanceSingleton,
  constants: constants,
  defaultExport: perfHooks,
};
}


function materializeNativeEntry(raw) {
  if (raw.entryType === 'resource') return globalThis.__wjsm_perf_emitNativeRaw(raw);
  globalThis.__wjsm_perf_enqueueNative(raw);
  return true;
}

const loadedPerfHooksModule = loadPerfHooksModule();
globalThis.__wjsm_node_perf_hooks.setNativeConverter(materializeNativeEntry);
globalThis.__wjsm_node_perf_hooks.setNativeDispatcher(globalThis.__wjsm_perf_dispatchNative);
const Performance = loadedPerfHooksModule.Performance;
const PerformanceEntry = loadedPerfHooksModule.PerformanceEntry;
const PerformanceMark = loadedPerfHooksModule.PerformanceMark;
const PerformanceMeasure = loadedPerfHooksModule.PerformanceMeasure;
const PerformanceObserver = loadedPerfHooksModule.PerformanceObserver;
const PerformanceObserverEntryList = loadedPerfHooksModule.PerformanceObserverEntryList;
const PerformanceResourceTiming = loadedPerfHooksModule.PerformanceResourceTiming;
const monitorEventLoopDelay = loadedPerfHooksModule.monitorEventLoopDelay;
const eventLoopUtilization = loadedPerfHooksModule.eventLoopUtilization;
const timerify = loadedPerfHooksModule.timerify;
const createHistogram = loadedPerfHooksModule.createHistogram;
const performance = loadedPerfHooksModule.performance;
const constants = loadedPerfHooksModule.constants;
const perfHooks = loadedPerfHooksModule.defaultExport;

export {
  Performance,
  PerformanceEntry,
  PerformanceMark,
  PerformanceMeasure,
  PerformanceObserver,
  PerformanceObserverEntryList,
  PerformanceResourceTiming,
  monitorEventLoopDelay,
  eventLoopUtilization,
  timerify,
  createHistogram,
  performance,
  constants,
};

export default perfHooks;
