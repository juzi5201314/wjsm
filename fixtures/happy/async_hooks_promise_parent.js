const { createHook } = require('node:async_hooks');

const promises = [];
const events = new Map();
const hook = createHook({
  init(asyncId, type, triggerAsyncId) {
    if (type !== 'PROMISE') return;
    promises.push({ asyncId, triggerAsyncId });
    events.set(asyncId, ['init']);
  },
  before(asyncId) {
    if (events.has(asyncId)) events.get(asyncId).push('before');
  },
  after(asyncId) {
    if (events.has(asyncId)) events.get(asyncId).push('after');
  },
  promiseResolve(asyncId) {
    if (events.has(asyncId)) events.get(asyncId).push('resolve');
  },
  trackPromises: true,
}).enable();

const first = Promise.resolve(1);
const second = first.then(() => 2);
second.then(() => {
  setImmediate(() => {
    hook.disable();
    console.log(promises.length >= 3);
    console.log(promises[1].triggerAsyncId === promises[0].asyncId);
    console.log(promises[2].triggerAsyncId === promises[1].asyncId);
    console.log(events.get(promises[0].asyncId).join(','));
    console.log(events.get(promises[1].asyncId).join(','));
    console.log(events.get(promises[2].asyncId).join(','));
  });
});
