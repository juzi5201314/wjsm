const specifier = './flaky' + '.js';
const stateSpecifier = './state' + '.js';
const id = require.resolve(specifier);

function tryRequire(label) {
  try {
    const loaded = require(specifier);
    console.log(label + ':loaded:' + (loaded.partial || loaded.done));
    return loaded;
  } catch (error) {
    console.log(label + ':error:' + (error.message.indexOf('boom-first') !== -1));
    return null;
  }
}

const first = tryRequire('first');
console.log(first === null);
console.log(require.cache[id] === undefined);

const second = tryRequire('second');
console.log(second === null);
console.log(require(stateSpecifier).count);
console.log(delete require.cache[id]);

const retry = tryRequire('retry');
console.log(retry.done);
console.log(retry.partial === undefined);
console.log(require(stateSpecifier).count);
console.log(require.cache[id].exports === retry);
