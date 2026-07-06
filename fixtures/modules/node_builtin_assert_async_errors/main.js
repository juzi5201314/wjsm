import { strictEqual, throws, doesNotThrow, rejects, doesNotReject, match, ifError } from 'node:assert';

try {
  strictEqual(1, 2);
} catch (err) {
  console.log(err.name, err.actual, err.expected, err.operator);
}

try {
  throws(() => {}, /x/);
} catch (err) {
  console.log(err.operator);
}

doesNotThrow(() => 1);

try {
  await rejects(Promise.resolve(1));
} catch (err) {
  console.log('rejects');
}


try {
  match('abc', /z/);
} catch (err) {
  console.log(err.operator);
}

try {
  ifError(new Error('boom'));
} catch (err) {
  console.log(err.operator);
}
