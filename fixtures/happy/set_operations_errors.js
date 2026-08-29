// Set 集合运算的错误路径：GetSetRecord（§24.2.1.2）校验、GetKeysIterator
//（§24.2.1.3）校验、incompatible receiver 渲染（文案对齐 Node/V8）。
const s = (it) => new Set(it);
const show = (v) => (v instanceof Set ? "Set(" + JSON.stringify([...v]) + ")" : JSON.stringify(v));
const t = (fn) => {
  try { console.log("OK", show(fn())); } catch (e) { console.log(e.constructor.name, e.message); }
};

// 参数必须是对象。
t(() => s([1]).union(null));
t(() => s([1]).union(7));
t(() => s([1]).intersection("x"));
// 数组不是 set-like（size undefined → NaN）。
t(() => s([1]).union([1, 2]));
// size 校验：NaN 抛 TypeError，负数抛 RangeError，getter 异常先传播。
t(() => s([1]).union({ size: NaN, has() {}, keys() {} }));
t(() => s([1]).union({ size: -1, has() {}, keys() {} }));
t(() => s([1]).union({ size: -Infinity, has() {}, keys() {} }));
t(() => s([1]).union({ get size() { throw new Error("sizeboom"); }, has() {}, keys() {} }));
// size 经 ToNumber（valueOf 参与）。
t(() => s([1]).union({ size: { valueOf() { return 2; } }, has() {}, keys() { return { next() { return { done: true }; } }; } }));
// has / keys 必须可调用。
t(() => s([1]).union({ size: 2, has: "x", keys() {} }));
t(() => s([1]).union({ size: 2, has() {}, keys: 5 }));
// keys() 结果必须是对象，next 必须可调用，next() 结果必须是对象。
t(() => s([1]).union({ size: 2, has() {}, keys() { return 3; } }));
t(() => s([1]).union({ size: 2, has() {}, keys() { return { next: 4 }; } }));
t(() => s([1]).union({ size: 2, has() {}, keys() { return { next() { return 5; } }; } }));
// 用户 has / next 抛出：原样传播。
t(() => s([1]).intersection({ size: 0, has() { throw new Error("hasboom"); }, keys() {} }));
t(() => s([1]).union({ size: 2, has() {}, keys() { return { next() { throw new Error("nextboom"); } }; } }));
// incompatible receiver：数组 / 普通对象 / Map / WeakMap / Promise / 原始值。
t(() => Set.prototype.union.call([], s([1])));
t(() => Set.prototype.union.call({}, s([1])));
t(() => Set.prototype.union.call(new Map(), s([1])));
t(() => Set.prototype.union.call(new WeakMap(), s([1])));
t(() => Set.prototype.union.call(Promise.resolve(), s([1])));
t(() => Set.prototype.union.call(7, s([1])));
t(() => Set.prototype.union.call("x", s([1])));
t(() => Set.prototype.union.call(null, s([1])));
t(() => Set.prototype.intersection.call(undefined, s([1])));
t(() => Set.prototype.difference.call([], s([1])));
t(() => Set.prototype.symmetricDifference.call([], s([1])));
t(() => Set.prototype.isSubsetOf.call([], s([1])));
t(() => Set.prototype.isSupersetOf.call([], s([1])));
t(() => Set.prototype.isDisjointFrom.call([], s([1])));
// isSupersetOf / isDisjointFrom 早退 false 时 IteratorClose：return 被调用。
{
  const ev = [];
  const like = {
    size: 1,
    has: () => true,
    keys() {
      const vals = [9, 8];
      let i = 0;
      return {
        next: () => (i < vals.length ? { value: vals[i++], done: false } : { done: true }),
        return() { ev.push("closed"); return { done: true }; },
      };
    },
  };
  console.log(s([1, 2]).isSupersetOf(like), JSON.stringify(ev));
  console.log(s([9]).isDisjointFrom(like), JSON.stringify(ev));
}
