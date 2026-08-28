// 对象解构对 null/undefined 的 RequireObjectCoercible（§13.15.5.2 /
// §8.6.2）：TypeError 文案对齐 V8 的上下文矩阵——顶层按源文本渲染
// callsite，嵌套按外层键名，参数/for-of/catch 用专属 callsite。

function probe(label, fn) {
  try {
    fn();
    console.log(label, "no-throw");
  } catch (error) {
    console.log(label, error instanceof TypeError, error.message);
  }
}

// 顶层声明：单键 / 多键 / 空模式 / rest / 默认值不改变检查。
probe("top-null", () => {
  const { a } = null;
  console.log(a);
});
probe("top-undefined", () => {
  const { a } = undefined;
  console.log(a);
});
probe("top-multi", () => {
  const { a, b } = null;
  console.log(a, b);
});
probe("top-empty", () => {
  const {} = null;
});
probe("top-rest", () => {
  const { ...rest } = null;
  console.log(rest);
});
probe("top-default", () => {
  const { a = 1 } = null;
  console.log(a);
});

// 顶层 callsite 源文本渲染：标识符 / 成员 / 调用。
probe("source-ident", () => {
  const value = null;
  const { x } = value;
  console.log(x);
});
probe("source-member", () => {
  const holder = { field: undefined };
  const { x } = holder.field;
  console.log(x);
});
probe("source-call", () => {
  function make() {
    return null;
  }
  const { x } = make();
  console.log(x);
});

// 嵌套：对象内嵌套报外层键，数组内嵌套报「'<idx>' of <源>」形态。
probe("nested-in-object", () => {
  const {
    outer: { inner },
  } = { outer: null };
  console.log(inner);
});
probe("nested-in-object-undefined", () => {
  const {
    outer: { inner },
  } = { outer: undefined };
  console.log(inner);
});
probe("nested-deep", () => {
  const {
    a: {
      b: { c },
    },
  } = { a: { b: null } };
  console.log(c);
});
probe("nested-in-array", () => {
  const [{ x }] = [null];
  console.log(x);
});
probe("nested-in-array-undefined", () => {
  const [{ x }] = [undefined];
  console.log(x);
});

// 赋值型解构与声明同一矩阵。
probe("assign-null", () => {
  let m;
  ({ m } = null);
  console.log(m);
});
probe("assign-array-nested", () => {
  let q;
  [{ q }] = [null];
  console.log(q);
});

// 函数参数：传 null 与缺省实参两种文案。
probe("param-null", () => {
  function take({ p }) {
    console.log(p);
  }
  take(null);
});
probe("param-missing", () => {
  function take({ p }) {
    console.log(p);
  }
  take();
});
probe("param-default-null", () => {
  function take({ p } = null) {
    console.log(p);
  }
  take();
});

// for-of 头与 catch 参数的专属 callsite。
probe("for-of-head", () => {
  for (const { z } of [null]) {
    console.log(z);
  }
});
probe("catch-param", () => {
  try {
    throw null;
  } catch ({ cause }) {
    console.log(cause);
  }
});
probe("catch-param-undefined", () => {
  try {
    throw undefined;
  } catch ({ cause }) {
    console.log(cause);
  }
});

// catch 参数解构 + finally：绑定初始化不受 finally 存在影响。
try {
  throw { tag: "caught-ok" };
} catch ({ tag }) {
  console.log("catch-binding", tag);
} finally {
  console.log("finally-ran");
}

// 非 nullish 基元可解构（ToObject 装箱）。
const { length } = "abc";
console.log("primitive-ok", length);
