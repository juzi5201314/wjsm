const { AsyncResource, createHook } = require('node:async_hooks');
const resource = new AsyncResource('instance');
console.log('instanceof', resource instanceof AsyncResource);
try { AsyncResource('direct'); } catch (error) { console.log('direct', error.name); }
console.log('empty-no-hook', typeof new AsyncResource('').asyncId);
const initHook = createHook({ init() {} }).enable();
try { new AsyncResource(''); } catch (error) { console.log('empty-with-hook', error.name); }
initHook.disable();

try { new AsyncResource(1); } catch (error) { console.log('type', error.code); }
try { new AsyncResource('x', true); } catch (error) { console.log('options', error.code); }
try { new AsyncResource('x', { triggerAsyncId: -2 }); } catch (error) {
  console.log('trigger-negative', error.code);
}
try { new AsyncResource('x', { triggerAsyncId: 1.5 }); } catch (error) {
  console.log('trigger-float', error.code);
}
try { new AsyncResource('x').runInAsyncScope(1); } catch (error) {
  console.log('run-fn', error.code);
}
try { new AsyncResource('x').bind(1); } catch (error) {
  console.log('bind-fn', error.code);
}
try { AsyncResource.bind(1); } catch (error) {
  console.log('static-bind-fn', error.code);
}
