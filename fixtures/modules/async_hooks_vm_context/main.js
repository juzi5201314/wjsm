const { AsyncResource, executionAsyncId } = require('async_hooks');
const vmName = 'vm';
const vm = require(vmName);
const sandbox = {};
vm.createContext(sandbox);
const resource = new AsyncResource('VMTEST');
resource.runInAsyncScope(() => {
  vm.runInContext(
    'globalThis.asyncId = globalThis.__wjsm_node_async_hooks.executionAsyncId()',
    sandbox,
  );
  console.log(sandbox.asyncId === resource.asyncId());
});
