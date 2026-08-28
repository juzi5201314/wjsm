// with 控制流与写入形态：break/continue/return/throw 穿越 with 体、
// update/delete、闭包捕获 with 环境、访问器属性、解构与 for-in/of 目标。
function find(items) {
  for (const item of items) {
    with (item) {
      if (stop) {
        break;
      }
      if (skip) {
        continue;
      }
      console.log(tag);
    }
  }
  return "done";
}
console.log(find([
  { tag: "a", stop: false, skip: false },
  { tag: "b", stop: false, skip: true },
  { tag: "c", stop: true, skip: false },
  { tag: "d", stop: false, skip: false },
]));

function early(o) {
  with (o) {
    return v * 2;
  }
}
console.log(early({ v: 21 }));

function makeCounter() {
  const box = { n: 0 };
  with (box) {
    return function () { return ++n; };
  }
}
const counter = makeCounter();
console.log(counter(), counter(), counter());

try {
  with ({ err: new Error("boom") }) {
    throw err;
  }
} catch (e) {
  console.log(e.message);
}

let updates = { n: 5 };
with (updates) {
  n++;
  console.log(n, --n);
}
console.log(updates.n);

const del = { d: 1 };
with (del) {
  console.log(delete d, "d" in del);
}

const t = { a: 0, b: 0 };
with (t) {
  ({ a } = { a: 1 });
  [b] = [2];
}
console.log(t.a, t.b);

const loop = { item: null, idx: null };
with (loop) {
  for (item of ["x", "y"]) {
    console.log(item);
  }
  for (idx in { k: 1 }) {
    console.log(idx);
  }
}
console.log(loop.item, loop.idx);

let backing = 10;
const accessor = {
  get val() { return backing; },
  set val(v) { backing = v * 2; },
};
with (accessor) {
  console.log(val);
  val = 21;
  console.log(val);
}
console.log(backing);

const fn = new Function("o", "with (o) { return marker; }");
console.log(fn({ marker: "fn-ok" }));
