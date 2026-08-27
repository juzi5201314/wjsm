// JSX spread 属性沿用 CopyDataProperties 异常语义：源求值抛错与
// 自有属性 getter 抛错都必须传播，不得静默产生残缺 props。
function boom(): never {
  throw new Error("jsx-boom");
}

try {
  const el = <div {...boom()} />;
  console.log("FAIL spread source");
} catch (e) {
  console.log("jsx spread source:", (e as Error).message);
}

const getterThrows = {
  get bad() {
    throw new Error("jsx-getter");
  },
};
try {
  const el = <div {...getterThrows} />;
  console.log("FAIL getter");
} catch (e) {
  console.log("jsx getter:", (e as Error).message);
}

// 嵌套子元素带 spread：异常分叉推进 block 后 children 仍正确收集
try {
  const el = (
    <div id="p">
      <span {...boom()} />
      <span id="after" />
    </div>
  );
  console.log("FAIL nested child");
} catch (e) {
  console.log("jsx nested child:", (e as Error).message);
}

// 正常 JSX spread 与嵌套 children 不受影响
const ok = (
  <div id="main" {...{ a: 1 }} b={2}>
    <span k="v" />
    text
  </div>
);
console.log("ok:", ok.type, JSON.stringify(ok.props), ok.children.length);
