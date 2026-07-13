const { createHook } = require('node:async_hooks');

const events = [];
const hooks = {};
const first = createHook({
  init(_id, type) {
    if (type === 'Timeout') {
      events.push('first');
      hooks.second.disable();
    }
  },
}).enable();
hooks.second = createHook({
  init(_id, type) {
    if (type === 'Timeout') events.push('second');
  },
}).enable();

setTimeout(() => {
  first.disable();
  console.log(events.join(','));
}, 0);
