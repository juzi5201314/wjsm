const { AsyncResource, createHook } = require('node:async_hooks');

const destroyed = [];
let autoId;
let manualId;
const hook = createHook({
  init(asyncId, type) {
    if (type === 'AUTO') autoId = asyncId;
    if (type === 'MANUAL') manualId = asyncId;
  },
  destroy(asyncId) {
    if (asyncId === autoId) destroyed.push('auto');
    if (asyncId === manualId) destroyed.push('manual');
  },
}).enable();

let automatic = new AsyncResource('AUTO');
let manual = new AsyncResource('MANUAL', { requireManualDestroy: true });
automatic = null;
manual = null;
gc();
setImmediate(() => {
  hook.disable();
  console.log(destroyed.join(','));
});
