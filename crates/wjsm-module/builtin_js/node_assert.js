import { isDeepStrictEqual } from 'node:util';

function sameValue(a, b) {
  if (a === b) return a !== 0 || 1 / a === 1 / b;
  return a !== a && b !== b;
}

export function AssertionError(options) {
  options = options || {};
  const message = options.message || (String(options.actual) + ' ' + options.operator + ' ' + String(options.expected));
  const err = new Error(message);
  err.name = 'AssertionError';
  err.actual = options.actual;
  err.expected = options.expected;
  err.operator = options.operator;
  return err;
}

function failWith(actual, expected, operator, message) {
  throw new AssertionError({ actual, expected, operator, message });
}

export function fail(message) {
  failWith(undefined, undefined, 'fail', message);
}
export function ok(value, message) {
  if (!value) failWith(value, true, '==', message);
}
function assert(value, message) { ok(value, message); }

export function equal(actual, expected, message) {
  if (actual != expected) failWith(actual, expected, '==', message);
}
export function notEqual(actual, expected, message) {
  if (actual == expected) failWith(actual, expected, '!=', message);
}
export function strictEqual(actual, expected, message) {
  if (!sameValue(actual, expected)) failWith(actual, expected, 'strictEqual', message);
}
export function notStrictEqual(actual, expected, message) {
  if (sameValue(actual, expected)) failWith(actual, expected, 'notStrictEqual', message);
}
export function deepStrictEqual(actual, expected, message) {
  if (!isDeepStrictEqual(actual, expected)) failWith(actual, expected, 'deepStrictEqual', message);
}
export const deepEqual = deepStrictEqual;
export function notDeepStrictEqual(actual, expected, message) {
  if (isDeepStrictEqual(actual, expected)) failWith(actual, expected, 'notDeepStrictEqual', message);
}
export const notDeepEqual = notDeepStrictEqual;

function matchesExpected(err, expected) {
  if (expected === undefined) return true;
  if (expected && typeof expected.test === 'function') return expected.test(String(err && err.message !== undefined ? err.message : err));
  if (typeof expected === 'function') {
    return expected(err) === true;
  }
  return false;
}

export function throws(fn, expected, message) {
  let thrown;
  try { fn(); } catch (err) { thrown = err; }
  if (thrown === undefined) failWith(undefined, expected, 'throws', message);
  if (!matchesExpected(thrown, expected)) throw thrown;
  return thrown;
}
export function doesNotThrow(fn, expected, message) {
  try {
    fn();
  } catch (err) {
    if (matchesExpected(err, expected)) failWith(err, expected, 'doesNotThrow', message);
    throw err;
  }
}
function promiseAssertion(actual, expected, operator, message) {
  return { name: 'AssertionError', actual, expected, operator, message };
}

export function rejects(promiseFn, expected, message) {
  let p;
  try {
    p = typeof promiseFn === 'function' ? promiseFn() : promiseFn;
  } catch (err) {
    if (!matchesExpected(err, expected)) return Promise.reject(err);
    return Promise.resolve(err);
  }
  return Promise.resolve(p).then(
    function onResolved(value) {
      return Promise.reject(promiseAssertion(value, expected, 'rejects', message));
    },
    function onRejected(err) {
      if (!matchesExpected(err, expected)) return Promise.reject(err);
      return err;
    }
  );
}
export function doesNotReject(promiseFn, expected, message) {
  let p;
  try {
    p = typeof promiseFn === 'function' ? promiseFn() : promiseFn;
  } catch (err) {
    if (matchesExpected(err, expected)) return Promise.reject(promiseAssertion(err, expected, 'doesNotReject', message));
    return Promise.reject(err);
  }
  return Promise.resolve(p);
}
export function match(string, regexp, message) {
  if (!regexp || typeof regexp.test !== 'function') throw new TypeError('regexp must be a RegExp');
  if (!regexp.test(String(string))) failWith(string, regexp, 'match', message);
}
export function doesNotMatch(string, regexp, message) {
  if (!regexp || typeof regexp.test !== 'function') throw new TypeError('regexp must be a RegExp');
  if (regexp.test(String(string))) failWith(string, regexp, 'doesNotMatch', message);
}
export function ifError(err) {
  if (err !== null && err !== undefined) failWith(err, null, 'ifError');
}

const assertDefault = {};
assertDefault.AssertionError = AssertionError;
assertDefault.ok = ok;
assertDefault.equal = equal;
assertDefault.notEqual = notEqual;
assertDefault.strictEqual = strictEqual;
assertDefault.notStrictEqual = notStrictEqual;
assertDefault.deepEqual = deepEqual;
assertDefault.deepStrictEqual = deepStrictEqual;
assertDefault.notDeepEqual = notDeepEqual;
assertDefault.notDeepStrictEqual = notDeepStrictEqual;
assertDefault.throws = throws;
assertDefault.doesNotThrow = doesNotThrow;
assertDefault.rejects = rejects;
assertDefault.doesNotReject = doesNotReject;
assertDefault.match = match;
assertDefault.doesNotMatch = doesNotMatch;
assertDefault.fail = fail;
assertDefault.ifError = ifError;
assertDefault.default = assertDefault;
export default assertDefault;
