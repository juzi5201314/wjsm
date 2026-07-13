const { executionAsyncId, triggerAsyncId } = require('async_hooks');
console.log(executionAsyncId());
console.log(triggerAsyncId());
