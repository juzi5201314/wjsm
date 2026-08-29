// 调用/构造 TypeError 的 callsite 表达式渲染（V8 CallPrinter 同型）：
// callee 非 callable / 非构造器时按源级表达式渲染文案（`o.foo is not a
// function`），与 Node v22 逐字节一致。语义层静态渲染经指令 callsite 元
// 数据与反馈槽编号传宿主拒绝路径；内部 desugar 站点保持按值渲染回退。
function show(label, fn) {
  try {
    fn();
    console.log(label, 'no-throw');
  } catch (e) {
    console.log(label, e.constructor.name, e.message);
  }
}

// ── 成员访问 callee ──
const o = { num: 5, str: 'x', nul: null, nested: { deep: 1 } };
show('member-dot', () => o.foo());
show('member-dot-num', () => o.num());
show('member-dot-null', () => o.nul());
show('member-str-key', () => o['bar']());
show('member-num-key', () => o[1]());
show('member-folded-key', () => o[1 + 2]());
var dynKey = 'dyn';
show('member-dyn-key', () => o[dynKey]());
var sym = Symbol('s');
show('member-sym-key', () => o[sym]());
show('member-deep', () => o.nested.deep());
show('member-space-key', () => o['a b']());

// ── 标识符 / 字面量 callee（解析期折叠） ──
var und;
show('ident-undefined', () => und());
var nil = null;
show('ident-null', () => nil());
show('lit-number', () => (1)());
show('lit-folded-add', () => (1 + 2)());
show('lit-string', () => 'str'());
show('lit-template', () => `tpl`());
show('lit-negative', () => (-1)());
show('lit-not', () => (!0)());
show('lit-exp', () => (1e21)());
show('lit-tiny', () => (1e-7)());
show('lit-bigint', () => (5n)());

// ── 复合表达式 callee ──
var a = 1, b = 2, c = 3;
show('nary-add', () => (a + b + c)());
show('nary-mixed', () => (a - b + c)());
show('logical-and', () => (a && b)());
show('seq', () => (a, b)());
show('cond', () => (a ? b : c)());
show('assign', () => (dynKey = 'k2')());
show('unary-typeof', () => (typeof a)());
show('update-prefix', () => (++a)());
show('update-postfix', () => (a++)());
show('array-lit', () => [a, b]());
show('object-lit', () => ({ p: 1 })());
show('template-sub', () => `x${a}y`());

// ── 调用结果 / 链式 ──
var mk = () => ({ m: 5 });
show('call-result-member', () => mk().m());
var mkFn = () => 5;
show('call-result-call', () => mkFn()());
show('paren-deep', () => (((o).foo))());

// ── 可选链 ──
show('optchain-call', () => o.num?.());
show('optchain-member', () => o?.num());
show('optchain-deep', () => o.nested?.deep?.());
show('optchain-paren', () => (o?.num)());

// ── tagged template ──
var tag = 5;
show('tagged', () => tag`quasi`);
show('tagged-member', () => o.num`quasi`);

// ── getter 结果 ──
var withGetter = { get g() { return 42; } };
show('getter-result', () => withGetter.g());

// ── 构造形态 ──
show('construct-ident', () => new und());
var five = 5;
show('construct-number', () => new five());
show('construct-member', () => new o.C());
show('construct-member-num', () => new o.num());
show('construct-folded', () => new (1 + 1)());
var inst = new (class {})();
show('construct-method', () => new inst.constructor.missing());

// ── intrinsic 慢路径（运行时改写后走通用调用） ──
globalThis.parseInt = 5;
show('intrinsic-global', () => parseInt('1'));
Object.keys = undefined;
show('intrinsic-static', () => Object.keys({}));
Array.prototype.map = null;
show('intrinsic-proto', () => [1].map(x => x));
globalThis.RegExp = 5;
show('intrinsic-construct', () => new RegExp('a'));

// ── with 作用域 ──
with ({ wfn: 5 }) {
  show('with-ident', () => wfn());
}

// ── 类上下文 ──
class Priv {
  #secret = 7;
  probe() { return this.#secret(); }
}
show('private-member', () => new Priv().probe());
class Base { }
class Derived extends Base {
  probe() { return super.absent(); }
}
show('super-member', () => new Derived().probe());

// ── 异步上下文（同步 try/catch 之外，微任务序确定） ──
(async () => {
  try {
    await o.missing();
  } catch (e) {
    console.log('async-member', e.constructor.name, e.message);
  }
  try {
    new o.MissingCtor();
  } catch (e) {
    console.log('async-construct', e.constructor.name, e.message);
  }
})();
