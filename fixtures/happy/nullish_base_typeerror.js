// 成员访问对 null/undefined 基座抛 TypeError（GetValue 步骤 3.a 的
// ToObject，§6.2.5.5 / §13.3.7.1），文案对齐 V8 三态：@@iterator 键报
// not iterable，基元键渲染进 (reading '<key>')，对象键省略后缀且
// TypeError 先于 ToPropertyKey（键转换副作用不得发生）。

function probe(label, fn) {
  try {
    fn();
    console.log(label, "no-throw");
  } catch (error) {
    console.log(label, error instanceof TypeError, error.message);
  }
}

// 读取：命名键 / 计算键 / 数字键 / Symbol 键。
probe("read-null-named", () => null.p);
probe("read-undefined-named", () => undefined.x);
probe("read-null-computed", () => null["k"]);
probe("read-undefined-index", () => undefined[0]);
const boxed = { inner: null };
probe("read-chained", () => boxed.inner.deep);
probe("read-symbol", () => null[Symbol("tag")]);
probe("read-async-iterator", () => undefined[Symbol.asyncIterator]);

// @@iterator 键：kNotIterableNoSymbolLoad 文案（callsite 按 typeof 前缀）。
probe("read-iterator-null", () => null[Symbol.iterator]);
probe("read-iterator-undefined", () => undefined[Symbol.iterator]);

// TypeError 先于 ToPropertyKey：对象键的 toString 不得执行，后缀省略。
let keyCoerced = false;
const spyKey = {
  toString() {
    keyCoerced = true;
    return "k";
  },
};
probe("read-object-key", () => null[spyKey]);
console.log("key-coerced", keyCoerced);

// 写入：命名 / 计算键，setting 文案。
probe("write-null-named", () => {
  null.p = 1;
});
probe("write-undefined-named", () => {
  undefined.q = 2;
});
probe("write-null-computed", () => {
  null["k"] = 3;
});

// 复合赋值 / 自增先读后写：报 reading 而非 setting。
probe("compound-null", () => {
  const target = null;
  target.p += 1;
});
probe("update-null", () => null.p++);

// delete 对 nullish 基座：ToObject 抛（§13.5.1.2 经 ToPropertyKey 前）。
probe("delete-null", () => delete null.p);

// GetIterator（§7.4.3）对 nullish：同步走 V8 回退 callsite 文案，
// 异步按普通属性读取渲染。V8 CallPrinter 可源文本渲染的形态
// （`for (x of null)` 等）见 iterator_nullish_callsite fixture。
probe("array-destructure-nested-null", () => {
  const [[first]] = [null];
  console.log(first);
});
async function probeAsync() {
  try {
    for await (const item of null) {
      console.log(item);
    }
  } catch (error) {
    console.log("for-await-null", error instanceof TypeError, error.message);
  }
}
probeAsync();
