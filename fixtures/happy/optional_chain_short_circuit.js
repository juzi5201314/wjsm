// 可选链链级短路（§13.3 OptionalExpression）：任一 `?.` 环基座为 nullish
// 时整条链短路产出 undefined——后续非可选环（成员/调用/实参/computed 键）
// 一律不求值；括号打断链，独立链的短路不外溢。

// 链级短路：`?.` 之后的普通 `.b` / 调用 / 键全部跳过。
console.log(null?.a);
console.log(null?.a.b);
console.log(undefined?.x.y);
console.log(null?.a.b.c());
console.log(null?.());
console.log(null?.().x);
console.log(null?.a?.b);
const nested = { a: null };
console.log(nested.a?.b.c);
console.log(nested?.a);

// 短路时 computed 键与实参不求值（§13.3.7.1 / §13.3.9.1）。
let keyRan = false;
const key = () => {
  keyRan = true;
  return "k";
};
console.log(null?.[key()], keyRan);
let argRan = false;
const arg = () => {
  argRan = true;
};
console.log(null?.f(arg()), argRan);
const fns = { f: null };
console.log(fns.f?.(arg()), argRan);

// 非短路路径：值、this 绑定、spread 实参照常。
const obj = {
  v: 1,
  m() {
    return this === obj;
  },
  inner: { w: 2 },
};
console.log(obj?.v, obj?.inner.w, obj?.m(), obj.m?.());
const chain = { a: () => ({ b: 9 }) };
console.log(chain?.a().b);
const spread = (...xs) => xs.length;
console.log(spread?.(...[1, 2, 3]));
const arr = [[10, 20]];
console.log(arr?.[0]?.[1]);

// 私有字段环：brand 在实例上 → 值；nullish 基座 → 链短路。
class Probe {
  #p = 7;
  read(target) {
    return target?.#p;
  }
}
const probe = new Probe();
console.log(probe.read(probe), probe.read(null), probe.read(undefined));

// 括号打断链：`(o?.a).b` 的 `.b` 是独立成员访问，nullish 时按 ToObject 抛。
const paren = { a: { b: 3 } };
console.log((paren?.a).b);
try {
  const broken = null;
  (broken?.a).b;
} catch (error) {
  console.log(error instanceof TypeError, error.message);
}

// 独立链的短路不外溢：computed 键内的 `y?.z` 短路成 undefined 后，
// 外层链继续以 "undefined" 为键读取。
const table = { undefined: 42 };
const missing = null;
console.log(table?.[missing?.z]);

// delete 可选链（§13.5.1.2）：短路恒 true；命中时删除属性；
// 调用环求值后恒 true。
const gone = null;
console.log(delete gone?.x);
const bag = { x: 1 };
console.log(delete bag?.x, "x" in bag);
console.log(delete null?.a.b.c);
let deleteCallRan = false;
console.log(delete ((() => {
  deleteCallRan = true;
})?.()), deleteCallRan);

// 短路后链值参与外围表达式。
console.log(typeof null?.a, (null?.a ?? "fallback"));
