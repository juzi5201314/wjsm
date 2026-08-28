// Iterator Helper 参数校验失败关闭底层迭代器（ECMA-262 现行 §27.1.4 各方法
// 步骤 3–4：对 NextMethod=undefined 的临时 Iterator Record 做 IteratorClose，
// 只读 return 不读 next）。Node v22 尚未落地该规范化（close on early errors），
// 本 fixture 以规范与 test262 iterator-helpers 为准，.expected 手工核对。

const closed = [];
function closable(name) {
  return {
    __proto__: Iterator.prototype,
    get next() { throw new Error(name + " next read"); },
    return() { closed.push(name); return {}; },
  };
}
try { closable("map").map(); } catch (e) { closed.push(e.constructor.name); }
try { closable("filter").filter({}); } catch (e) { closed.push(e.constructor.name); }
try { closable("flatMap").flatMap(1); } catch (e) { closed.push(e.constructor.name); }
try { closable("take-nan").take(); } catch (e) { closed.push(e.constructor.name + ":" + e.message); }
try { closable("take-neg").take(-1); } catch (e) { closed.push(e.constructor.name); }
try { closable("drop-throw").drop({ get valueOf() { throw new Error("custom"); } }); } catch (e) { closed.push(e.message); }
try { closable("reduce").reduce(); } catch (e) { closed.push(e.constructor.name); }
try { closable("forEach").forEach("x"); } catch (e) { closed.push(e.constructor.name); }
try { closable("some").some(null); } catch (e) { closed.push(e.constructor.name); }
try { closable("every").every(0); } catch (e) { closed.push(e.constructor.name); }
try { closable("find").find(undefined); } catch (e) { closed.push(e.constructor.name); }
console.log(closed.join("|"));

// close 期 return 抛出被 throw 完成吞掉，原始 TypeError 照常传播（§7.4.11）。
try {
  Iterator.prototype.map.call({ return() { throw new Error("swallowed"); } });
} catch (e) {
  console.log("throwing return: " + e.constructor.name);
}

// 校验通过时不触发 close：合法 limit 直达 GetIteratorDirect。
const untouched = [];
const okDrop = Iterator.prototype.drop.call(
  { next() { return { done: true }; }, return() { untouched.push("return"); return {}; } },
  0,
);
console.log("valid limit: " + JSON.stringify(okDrop.next()) + " " + untouched.length);

// drop 跳过阶段按 IteratorStep 只读 done 不读 value（§7.4.9）。
let valueReads = 0;
let steps = 0;
const dropSkip = Iterator.prototype.drop.call(
  {
    next() {
      steps++;
      return { done: false, get value() { valueReads++; return steps; } };
    },
  },
  2,
);
console.log("drop skip: " + dropSkip.next().value + " steps " + steps + " value reads " + valueReads);
