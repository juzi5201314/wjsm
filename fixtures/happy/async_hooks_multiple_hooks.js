const { createHook } = require('node:async_hooks');

const events = [];
const first = createHook({
  init(_id, type) {
    if (type === 'Timeout') events.push('first');
  },
}).enable();
const second = createHook({
  init(_id, type) {
    if (type === 'Timeout') events.push('second');
  },
}).enable();
setTimeout(() => {
  first.disable();
  second.disable();
  console.log(events.join(','));
}, 0);
