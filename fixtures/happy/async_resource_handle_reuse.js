const { AsyncResource, createHook } = require('node:async_hooks');

let created = 0;
let initialized = 0;
const hook = createHook({
  init(asyncId, type) {
    if (type === 'HANDLE_REUSE') initialized++;
  },
}).enable();

function runBatch() {
  const end = created + 350;
  while (created < end) {
    new AsyncResource('HANDLE_REUSE').emitDestroy();
    const garbage = [{}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}];
    if (garbage.length !== 24) throw new Error('allocation failed');
    created++;
  }
  gc();
  if (created < 700) {
    setImmediate(runBatch);
    return;
  }
  hook.disable();
  console.log(created === 700 && initialized === 700);
}

runBatch();
