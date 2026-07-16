const {
  PerformanceEntry,
  PerformanceObserver,
  PerformanceResourceTiming,
  constants,
} = require('node:perf_hooks');
const http = require('node:http');
const net = require('node:net');

const requiredEntryTypes = ['gc', 'net', 'http', 'resource'];
const entries = [];
const seenTypes = { gc: false, net: false, http: false, resource: false };
let expectedFailures = 0;
let finished = false;
let netPort = 0;
let netServer;

const observer = new PerformanceObserver((list) => {
  const batch = list.getEntries();
  for (let i = 0; i < batch.length; i = i + 1) {
    entries.push(batch[i]);
    seenTypes[batch[i].entryType] = true;
  }
  finishIfReady();
});
observer.observe({ entryTypes: requiredEntryTypes });

function finishIfReady() {
  const hasRequiredEntries =
    seenTypes.gc && seenTypes.net && seenTypes.http && seenTypes.resource;
  if (finished || expectedFailures !== 2 || !hasRequiredEntries) return;

  emitAssertions();
}

function emitAssertions() {
  if (finished) return;
  finished = true;
  const gcEntries = entries.filter((entry) => entry.entryType === 'gc');
  const forcedGc = gcEntries.find(
    (entry) => entry.detail.flags === constants.NODE_PERFORMANCE_GC_FLAGS_FORCED
  );
  console.log(
    Boolean(
      forcedGc &&
      forcedGc instanceof PerformanceEntry &&
      forcedGc.name === 'gc' &&
      forcedGc.detail.kind === constants.NODE_PERFORMANCE_GC_MAJOR &&
      forcedGc.startTime >= 0 &&
      forcedGc.duration >= 0
    )
  );

  const netEntries = entries.filter((entry) => entry.entryType === 'net');
  const connect = netEntries[0];
  console.log(
    netEntries.length === 1 &&
    connect instanceof PerformanceEntry &&
    connect.name === 'connect' &&
    connect.detail.host === '127.0.0.1' &&
    connect.detail.port === netPort &&
    connect.startTime >= 0 &&
    connect.duration >= 0 &&
    expectedFailures === 2
  );

  const httpEntries = entries.filter((entry) => entry.entryType === 'http');
  const client = httpEntries[0];
  console.log(
    httpEntries.length === 1 &&
    client instanceof PerformanceEntry &&
    client.name === 'HttpClient' &&
    client.detail.req.method === 'GET' &&
    client.detail.req.url === 'data:text/plain,hello' &&
    client.detail.res.statusCode === 200 &&
    expectedFailures === 2
  );

  const resources = entries.filter((entry) => entry.entryType === 'resource');
  console.log(
    resources.length === 1 &&
    resources[0] instanceof PerformanceResourceTiming &&
    resources[0].name === 'data:text/plain,hello' &&
    resources[0].initiatorType === 'fetch' &&
    resources[0].responseStatus === 200 &&
    resources[0].encodedBodySize === 5 &&
    resources[0].decodedBodySize === 5
  );

  observer.disconnect();
  process.exit(0);
}

function recordNetFailure() {
  expectedFailures = expectedFailures + 1;
  setImmediate(runFailedFetch);
}

function runFailedFetch() {
  fetch('http://127.0.0.1:1/unreachable').catch(recordFetchFailure);
}

function recordFetchFailure() {
  expectedFailures = expectedFailures + 1;
  setImmediate(runHttpClient);
  finishIfReady();
}

function runFailedProducers() {
  const failedSocket = net.connect(1, '127.0.0.1');
  failedSocket.on('error', recordNetFailure);
}


function finishRequestChain() {
  finishIfReady();
}

function consumeFetchResponse(response) {
  return response.text();
}

function runFetch() {
  fetch('data:text/plain,hello').then(consumeFetchResponse).then(runFailedProducers);
}

function runHttpClient() {
  const request = http.get('data:text/plain,hello', finishRequestChain);
  request.on('error', () => {});
}

netServer = net.createServer((socket) => socket.end());
netServer.listen(0, '127.0.0.1', () => {
  netPort = netServer.address().port;
  let client;
  client = net.connect(netPort, '127.0.0.1', () => {
    client.destroy();
    setImmediate(runFetch);
    gc();
  });
});
