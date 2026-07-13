const { createHook } = require('node:async_hooks');

for (const [name, options] of [
  ['options', null],
  ['init', { init: 1 }],
  ['before', { before: null }],
  ['after', { after: 'x' }],
  ['destroy', { destroy: {} }],
  ['promiseResolve', { promiseResolve: true }],
  ['trackPromises', { trackPromises: 1 }],
]) {
  try {
    createHook(options);
    console.log(name, 'accepted');
  } catch (error) {
    console.log(name, error.code);
  }
}

const hook = createHook({ trackPromises: true });
console.log(hook.enable() === hook, hook.disable() === hook);
