// GetIterator（§7.4.3）对 nullish 值的 TypeError 文案：wjsm 统一使用 V8 的
// BuildDefaultCallSite 回退形态「<typeof 前缀> is not iterable (cannot read
// property Symbol(Symbol.iterator))」。V8 在源文本可打印时经 CallPrinter 渲染
// 为「null is not iterable」/「o.missing is not iterable」，该源文本渲染
// wjsm 未实现——本 fixture 的期望是 wjsm 专有回退文案（TypeError 类型与
// 抛出时机与 Node 一致，仅 callsite 渲染不同）。

function probe(label, fn) {
  try {
    fn();
    console.log(label, "no-throw");
  } catch (error) {
    console.log(label, error instanceof TypeError, error.message);
  }
}

probe("for-of-null", () => {
  for (const item of null) {
    console.log(item);
  }
});
probe("for-of-undefined", () => {
  for (const item of undefined) {
    console.log(item);
  }
});
probe("spread-null", () => [...null]);
probe("spread-undefined", () => [...undefined]);
probe("top-destructure-null", () => {
  const [first] = null;
  console.log(first);
});
