const { createHook } = require('node:async_hooks');

const promises = new Map();
let active = 0;
let beforeCount = 0;
let afterCount = 0;
const hook = createHook({
  init(asyncId, type) {
    if (type === 'PROMISE') promises.set(asyncId, true);
  },
  before(asyncId) {
    if (!promises.has(asyncId)) return;
    active++;
    beforeCount++;
  },
  after(asyncId) {
    if (!promises.has(asyncId)) return;
    active--;
    afterCount++;
  },
}).enable();

Promise.all([Promise.resolve(1)])
  .finally(() => {})
  .then(() => {
    setImmediate(() => {
      hook.disable();
      console.log(active === 0);
      console.log(beforeCount > 0 && beforeCount === afterCount);
    });
  });
