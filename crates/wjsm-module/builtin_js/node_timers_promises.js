function createAbortError() {
  const error = new Error('The operation was aborted');
  error.name = 'AbortError';
  error.code = 'ABORT_ERR';
  return error;
}

function signalOf(options) {
  if (options === undefined || options === null) return undefined;
  return options.signal;
}

function onAbort(signal, listener) {
  if (signal && typeof signal.addEventListener === 'function') {
    signal.addEventListener('abort', listener, { once: true });
    return true;
  }
  return false;
}

function offAbort(signal, listener) {
  if (signal && typeof signal.removeEventListener === 'function') {
    signal.removeEventListener('abort', listener);
  }
}

function promisesSetTimeout(delay, value, options) {
  const signal = signalOf(options);
  return new Promise((resolve, reject) => {
    if (signal && signal.aborted) {
      reject(createAbortError());
      return;
    }
    const abortListener = () => {
      clearTimeout(handle);
      reject(createAbortError());
    };
    const handle = setTimeout(() => {
      offAbort(signal, abortListener);
      resolve(value);
    }, delay);
    onAbort(signal, abortListener);
  });
}

function promisesSetImmediate(value, options) {
  const signal = signalOf(options);
  return new Promise((resolve, reject) => {
    if (signal && signal.aborted) {
      reject(createAbortError());
      return;
    }
    const abortListener = () => {
      clearImmediate(handle);
      reject(createAbortError());
    };
    const handle = setImmediate(() => {
      offAbort(signal, abortListener);
      resolve(value);
    });
    onAbort(signal, abortListener);
  });
}

async function* promisesSetInterval(delay, value, options) {
  const signal = signalOf(options);
  while (true) {
    if (signal && signal.aborted) throw createAbortError();
    await promisesSetTimeout(delay, undefined, options);
    yield value;
  }
}

export const scheduler = {
  wait(delay, options) {
    return promisesSetTimeout(delay, undefined, options);
  },
  yield() {
    return promisesSetImmediate();
  },
};

export {
  promisesSetTimeout as setTimeout,
  promisesSetImmediate as setImmediate,
  promisesSetInterval as setInterval,
};
export default {
  setTimeout: promisesSetTimeout,
  setImmediate: promisesSetImmediate,
  setInterval: promisesSetInterval,
  scheduler,
};
