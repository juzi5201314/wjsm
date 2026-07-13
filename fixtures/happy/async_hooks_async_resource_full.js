const {
  AsyncResource,
  createHook,
  executionAsyncId,
  executionAsyncResource,
  triggerAsyncId,
} = require('node:async_hooks');

const rootResource = executionAsyncResource();
console.log(typeof rootResource, rootResource === executionAsyncResource());

const events = [];
let targetId;
const hook = createHook({
  init(asyncId, type, trigger) {
    if (type === 'CUSTOM') {
      targetId = asyncId;
      events.push('init:' + trigger);
    }
  },
  before(asyncId) {
    if (asyncId === targetId) events.push('before');
  },
  after(asyncId) {
    if (asyncId === targetId) events.push('after');
  },
  destroy(asyncId) {
    if (asyncId === targetId) events.push('destroy');
  },
}).enable();

const resource = new AsyncResource('CUSTOM', { triggerAsyncId: 1 });
console.log(resource.asyncId() === targetId, resource.triggerAsyncId() === 1);
const value = resource.runInAsyncScope(function (a, b) {
  console.log(this.name, a + b);
  console.log(executionAsyncId() === targetId, triggerAsyncId() === 1);
  console.log(executionAsyncResource() === resource);
  return a * b;
}, { name: 'scope' }, 2, 3);
console.log(value);

const bound = resource.bind(function (value) {
  console.log(this.name, executionAsyncId() === targetId);
  return value + 1;
}, { name: 'bound' });
console.log(bound(4), bound.length);
resource.emitDestroy().emitDestroy();
setImmediate(() => {
  hook.disable();
  console.log(events.join(','));
});
