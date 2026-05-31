// fetch_headers_constructor_init
// Covers: new Headers(), new Headers(record), new Headers(sequence), new Headers(existing Headers copy)
// duplicate names joined by ", " on get(), set replaces, copies independent

function assertEq(actual, expected, msg) {
  if (actual !== expected) {
    console.log("FAIL", msg, "got", actual, "want", expected);
    throw new Error("assert");
  }
}

const hEmpty = new Headers();
console.log("empty has x:", hEmpty.has("x-foo"));

const record = { "X-Foo": "bar", "x-bar": "baz" };
const hRec = new Headers(record);
console.log("rec has x-foo:", hRec.has("x-foo"));
console.log("rec get x-foo:", hRec.get("x-foo"));

const seq = [["x-dup", "v1"], ["X-DUP", "v2"], ["x-other", "single"]];
const hSeq = new Headers(seq);
console.log("seq get x-dup:", hSeq.get("x-dup"));  // "v1, v2" per spec+fixture convention

const hCopy = new Headers(hSeq);
console.log("copy get x-dup:", hCopy.get("x-dup"));

hSeq.set("x-dup", "replaced");
console.log("orig after set get x-dup:", hSeq.get("x-dup"));
console.log("copy independent get x-dup:", hCopy.get("x-dup"));  // still "v1, v2"

hSeq.append("x-dup", "extra");
console.log("after append get:", hSeq.get("x-dup"));  // "replaced, extra"

console.log("done headers ctor");
