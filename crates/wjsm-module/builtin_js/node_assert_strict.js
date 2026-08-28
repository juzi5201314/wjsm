import {
  AssertionError as baseAssertionError,
  deepStrictEqual as baseDeepStrictEqual,
  doesNotMatch as baseDoesNotMatch,
  doesNotReject as baseDoesNotReject,
  doesNotThrow as baseDoesNotThrow,
  fail as baseFail,
  ifError as baseIfError,
  match as baseMatch,
  notDeepStrictEqual as baseNotDeepStrictEqual,
  notStrictEqual as baseNotStrictEqual,
  ok as baseOk,
  rejects as baseRejects,
  strictEqual as baseStrictEqual,
  throws as baseThrows,
} from 'node:assert';

function strictAssert(value, message) {
  baseOk(value, message);
}

strictAssert.AssertionError = baseAssertionError;
strictAssert.fail = baseFail;
strictAssert.ok = baseOk;
strictAssert.equal = baseStrictEqual;
strictAssert.notEqual = baseNotStrictEqual;
strictAssert.deepEqual = baseDeepStrictEqual;
strictAssert.notDeepEqual = baseNotDeepStrictEqual;
strictAssert.strictEqual = baseStrictEqual;
strictAssert.notStrictEqual = baseNotStrictEqual;
strictAssert.deepStrictEqual = baseDeepStrictEqual;
strictAssert.notDeepStrictEqual = baseNotDeepStrictEqual;
strictAssert.throws = baseThrows;
strictAssert.doesNotThrow = baseDoesNotThrow;
strictAssert.rejects = baseRejects;
strictAssert.doesNotReject = baseDoesNotReject;
strictAssert.match = baseMatch;
strictAssert.doesNotMatch = baseDoesNotMatch;
strictAssert.ifError = baseIfError;
strictAssert.strict = strictAssert;

export const AssertionError = baseAssertionError;
export const fail = baseFail;
export const ok = baseOk;
export const equal = baseStrictEqual;
export const notEqual = baseNotStrictEqual;
export const deepEqual = baseDeepStrictEqual;
export const notDeepEqual = baseNotDeepStrictEqual;
export const strictEqual = baseStrictEqual;
export const notStrictEqual = baseNotStrictEqual;
export const deepStrictEqual = baseDeepStrictEqual;
export const notDeepStrictEqual = baseNotDeepStrictEqual;
export const throws = baseThrows;
export const doesNotThrow = baseDoesNotThrow;
export const rejects = baseRejects;
export const doesNotReject = baseDoesNotReject;
export const match = baseMatch;
export const doesNotMatch = baseDoesNotMatch;
export const ifError = baseIfError;
export const strict = strictAssert;
export default strictAssert;
