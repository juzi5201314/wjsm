const {
  PerformanceObserver,
  PerformanceResourceTiming,
  performance,
} = require("node:perf_hooks");

function timing(startTime) {
  return {
    startTime,
    redirectStartTime: startTime + 1,
    redirectEndTime: startTime + 2,
    postRedirectStartTime: startTime + 3,
    finalServiceWorkerStartTime: startTime + 4,
    finalNetworkRequestStartTime: startTime + 10,
    finalNetworkResponseStartTime: startTime + 20,
    endTime: startTime + 40,
    encodedBodySize: 100,
    decodedBodySize: 200,
    finalConnectionTimingInfo: {
      domainLookupStartTime: startTime + 5,
      domainLookupEndTime: startTime + 6,
      connectionStartTime: startTime + 7,
      connectionEndTime: startTime + 8,
      secureConnectionStartTime: startTime + 7.5,
      ALPNNegotiatedProtocol: "h2",
    },
  };
}

performance.clearResourceTimings();
const entry = performance.markResourceTiming(
  timing(10),
  "https://example.test/exact",
  "fetch",
  globalThis,
  "",
  {},
  201,
  "cache"
);
console.log(
  entry instanceof PerformanceResourceTiming &&
    entry.name === "https://example.test/exact" &&
    entry.entryType === "resource" &&
    entry.startTime === 10 &&
    entry.duration === 40 &&
    entry.initiatorType === "fetch" &&
    entry.workerStart === 14 &&
    entry.redirectStart === 11 &&
    entry.redirectEnd === 12 &&
    entry.fetchStart === 13 &&
    entry.domainLookupStart === 15 &&
    entry.domainLookupEnd === 16 &&
    entry.connectStart === 17 &&
    entry.connectEnd === 18 &&
    entry.secureConnectionStart === 17.5 &&
    entry.nextHopProtocol === "h2" &&
    entry.requestStart === 20 &&
    entry.responseStart === 30 &&
    entry.responseEnd === 50 &&
    entry.transferSize === 400 &&
    entry.encodedBodySize === 100 &&
    entry.decodedBodySize === 200 &&
    entry.deliveryType === "cache" &&
    entry.responseStatus === 201
);

const json = entry.toJSON();
console.log(
  Object.keys(json).join(",") ===
    "name,entryType,startTime,duration,initiatorType,nextHopProtocol,workerStart,redirectStart,redirectEnd,fetchStart,domainLookupStart,domainLookupEnd,connectStart,connectEnd,secureConnectionStart,requestStart,responseStart,responseEnd,transferSize,encodedBodySize,decodedBodySize,deliveryType,responseStatus"
);

performance.clearResourceTimings();
performance.setResourceTimingBufferSize(2);
let bufferFullEvents = 0;
const onBufferFull = () => {
  bufferFullEvents += 1;
};
performance.addEventListener("resourcetimingbufferfull", onBufferFull);

const observed = [];
const observer = new PerformanceObserver((list) => {
  const entries = list.getEntriesByType("resource");
  for (let index = 0; index < entries.length; index += 1) {
    observed.push(entries[index]);
  }
});
observer.observe({ entryTypes: ["resource"] });

for (let index = 0; index < 3; index += 1) {
  performance.markResourceTiming(
    timing(index + 1),
    "resource-" + index,
    "fetch",
    globalThis,
    "local",
    {},
    200,
    ""
  );
}

setImmediate(() => {
  const timeline = performance.getEntriesByType("resource");
  console.log(
    timeline.length === 2 &&
      timeline[0].name === "resource-0" &&
      timeline[1].name === "resource-1" &&
      timeline[0].transferSize === 0
  );
  console.log(
    observed.length === 3 &&
      observed[2].name === "resource-2" &&
      bufferFullEvents === 1
  );
  observer.disconnect();
  performance.removeEventListener("resourcetimingbufferfull", onBufferFull);
  performance.clearResourceTimings();
});
