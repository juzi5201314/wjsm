const { PerformanceObserver } = require('node:perf_hooks');
const http = require('node:http');

let sawResponse = false;
let sawEntry = false;

let observer;
function finish() {
  if (!sawResponse || !sawEntry) return;
  console.log(true);
  observer.disconnect();
}

observer = new PerformanceObserver((list) => {
  const entry = list.getEntriesByType('http')[0];
  sawEntry = Boolean(entry && entry.name === 'HttpClient');
  finish();
});
observer.observe({ entryTypes: ['gc', 'net', 'http', 'resource'] });
gc();

http.get('data:text/plain,hello', () => {
  sawResponse = true;
  finish();
});
