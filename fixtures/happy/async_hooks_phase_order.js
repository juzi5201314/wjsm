const { createHook } = require('node:async_hooks');

const idsByType = Object.create(null);
const events = new Map();
const selected = new Set();
const hook = createHook({
  init(asyncId, type) {
    if (type !== 'TickObject' && type !== 'Timeout' && type !== 'Immediate') return;
    if (idsByType[type] !== undefined) return;
    idsByType[type] = asyncId;
    selected.add(asyncId);
    events.set(asyncId, ['init']);
  },
  before(asyncId) {
    if (selected.has(asyncId)) events.get(asyncId).push('before');
  },
  after(asyncId) {
    if (selected.has(asyncId)) events.get(asyncId).push('after');
  },
  destroy(asyncId) {
    if (selected.has(asyncId)) events.get(asyncId).push('destroy');
  },
}).enable();

process.nextTick(() => {});
setTimeout(() => {}, 0);
setImmediate(() => {});
setTimeout(() => {
  hook.disable();
  for (const type of ['TickObject', 'Timeout', 'Immediate']) {
    const asyncId = idsByType[type];
    console.log(type, asyncId === undefined ? 'missing' : events.get(asyncId).join(','));
  }
}, 20);
