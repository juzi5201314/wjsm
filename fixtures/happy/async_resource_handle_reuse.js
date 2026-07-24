const { AsyncResource, createHook } = require('node:async_hooks');

let created = 0;
let initialized = 0;
const hook = createHook({
  init(asyncId, type) {
    if (type === 'HANDLE_REUSE') initialized++;
  },
}).enable();

// 正确性验证：每个 AsyncResource 创建即触发一次 init，emitDestroy + gc 不丢/不重 init 计数。
// 总数对语义是任意的（handle 复用语义由 Rust 侧单测覆盖），取一批即可验证计数一致。
const TOTAL = 70;

function runBatch() {
  const end = created + TOTAL;
  while (created < end) {
    new AsyncResource('HANDLE_REUSE').emitDestroy();
    const garbage = [{}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}];
    if (garbage.length !== 24) throw new Error('allocation failed');
    created++;
  }
  gc();
  hook.disable();
  console.log(created === TOTAL && initialized === TOTAL);
}

runBatch();
