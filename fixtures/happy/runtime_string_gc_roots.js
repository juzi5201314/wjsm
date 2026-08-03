function dynamic(left, right) {
  return [left, right].join("");
}

function churn() {
  for (let i = 0; i < 3000; i++) {
    const temporary = "garbage-" + i + "-" + (i + 1);
    if (temporary === "unreachable") console.log(temporary);
  }
}

function localRoot() {
  const value = dynamic("local", "-root");
  churn();
  gc();
  return value;
}

const object = { value: dynamic("object", "-root") };
const array = [dynamic("array", "-root")];
const closureValue = dynamic("closure", "-root");
const closure = (() => {
  const captured = closureValue;
  return () => captured;
})();
const boundValue = dynamic("bound", "-root");
const bound = ((value) => value).bind(null, boundValue);

globalThis.runtimeStringGcInner = new Promise((resolve) => {
  globalThis.runtimeStringGcResolve = resolve;
});
const finallyValue = dynamic("finally", "-root");
globalThis.runtimeStringGcFinally = Promise.resolve(finallyValue).finally(() => {
  queueMicrotask(() => {
    churn();
    gc();
    globalThis.runtimeStringGcResolve("ignored");
  });
  return globalThis.runtimeStringGcInner;
});
globalThis.runtimeStringGcFinally.then((value) => console.log(value));

console.log(localRoot());
churn();
gc();
console.log(object.value);
console.log(array[0]);
console.log(closure());
console.log(bound());

try {
  throw dynamic("exception", "-root");
} catch (error) {
  churn();
  gc();
  console.log(error);
}

