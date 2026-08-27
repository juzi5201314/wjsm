// 身份逃逸时不得标量替换：通过另一对象观察变异。
const RECORD = { name: 1, value: 2, length: 3 };
const leak = {};
leak.inner = RECORD;
RECORD.name = 9;
console.log(leak.inner.name, leak.inner.value, leak.inner.length);
