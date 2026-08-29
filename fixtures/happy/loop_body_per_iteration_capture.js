// 循环体内词法绑定按迭代实例化的行为矩阵：六种循环 × 混合捕获 ×
// TDZ / 变更可见性 / 嵌套结构，全部与 Node 输出逐字节对拍。

// 只捕获体内 const（不混合外层绑定）
{
  const fns = [];
  for (let i = 0; i < 3; i++) {
    const k = i;
    fns.push(() => k);
  }
  console.log("t1", fns.map((f) => f()).join(","));
}

// do-while 体内 const + 外层捕获
{
  const fns = [];
  let label = "D";
  let j = 0;
  do {
    const k = j;
    fns.push(() => label + k);
    j++;
  } while (j < 3);
  console.log("t2", fns.map((f) => f()).join(","));
}

// while 体内 const + 外层捕获
{
  const fns = [];
  const label = "W";
  let j = 0;
  while (j < 3) {
    const k = j * 2;
    fns.push(() => label + k);
    j++;
  }
  console.log("t3", fns.map((f) => f()).join(","));
}

// for-in 头部 const + 体内 const + 外层捕获
{
  const fns = [];
  const label = "I";
  for (const p in { a: 1, b: 2 }) {
    const k = p + "!";
    fns.push(() => label + p + k);
  }
  console.log("t4", fns.map((f) => f()).join(","));
}

// for-of 头部 const + 体内 const + 外层捕获
{
  const fns = [];
  const label = "O";
  for (const v of [10, 20]) {
    const k = v + 1;
    fns.push(() => label + v + ":" + k);
  }
  console.log("t5", fns.map((f) => f()).join(","));
}

// 体内 let 捕获后同轮变更：闭包见同轮实例的新值
{
  const fns = [];
  for (let i = 0; i < 3; i++) {
    let m = i;
    fns.push(() => m);
    m = i * 100;
  }
  console.log("t6", fns.map((f) => f()).join(","));
}

// 同轮两个闭包共享同一绑定实例；跨轮实例互不影响
{
  const pairs = [];
  for (let i = 0; i < 2; i++) {
    let m = i;
    pairs.push([() => m, (v) => { m = v; }]);
  }
  pairs[0][1](42);
  console.log("t7", pairs[0][0](), pairs[1][0]());
}

// 嵌套循环：外层体 const 与内层体 const 各按各自迭代实例化
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    const a = i * 10;
    for (let j = 0; j < 2; j++) {
      const b = a + j;
      fns.push(() => a + ":" + b);
    }
  }
  console.log("t8", fns.map((f) => f()).join(","));
}

// 体内同名兄弟块各自独立实例
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    { const k = "a" + i; fns.push(() => k); }
    { const k = "b" + i; fns.push(() => k); }
  }
  console.log("t9", fns.map((f) => f()).join(","));
}

// catch 参数是按次执行的块级绑定，循环内被闭包捕获须按迭代
{
  const fns = [];
  const label = "C";
  for (let i = 0; i < 3; i++) {
    try {
      throw i;
    } catch (e) {
      fns.push(() => label + e);
    }
  }
  console.log("t10", fns.map((f) => f()).join(","));
}

// 稳定外层 let 循环后重赋值：全部闭包见新值（活绑定不受按迭代影响）
{
  const fns = [];
  let label = "old";
  for (let i = 0; i < 2; i++) {
    const k = i;
    fns.push(() => label + k);
  }
  label = "new";
  console.log("t11", fns.map((f) => f()).join(","));
}

// 循环体内类声明按迭代捕获外层循环变量
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    class P {
      constructor() {
        this.v = i;
      }
    }
    fns.push(() => new P().v);
  }
  console.log("t12", fns[0](), fns[1]());
}

// switch case 内 const 捕获
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    switch (i) {
      default: {
        const k = i + 100;
        fns.push(() => k);
      }
    }
  }
  console.log("t13", fns.map((f) => f()).join(","));
}

// TDZ 前向捕获：闭包先于声明创建，循环后调用见各轮值
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    fns.push(() => k);
    const k = i * 5;
  }
  console.log("t14", fns[0](), fns[1]());
}

// TDZ 轮内先调用抛 ReferenceError，声明后再调用见值
{
  for (let i = 0; i < 1; i++) {
    const f = () => k;
    try {
      f();
    } catch (e) {
      console.log("t15", e.constructor.name);
    }
    const k = 9;
    console.log("t15b", f());
  }
}

// while 体 TDZ 前向捕获
{
  let j = 0;
  const fns = [];
  while (j < 2) {
    fns.push(() => q + j);
    const q = j * 7;
    j++;
  }
  console.log("t16", fns[0](), fns[1]());
}

// 闭包对体内 let 写入：同轮内经 env 定位同一实例
{
  const out = [];
  for (let i = 0; i < 2; i++) {
    let acc = 0;
    const add = (n) => { acc += n; };
    add(i);
    add(10);
    out.push(acc);
  }
  console.log("t17", out[0], out[1]);
}

// labeled continue 跳过声明后半段：仅未跳过轮次入列
{
  const fns = [];
  outer: for (let i = 0; i < 3; i++) {
    const k = i;
    if (i === 1) continue outer;
    fns.push(() => k);
  }
  console.log("t18", fns.map((f) => f()).join(","));
}

// 对象字面量方法与 getter 捕获体内 const
{
  const objs = [];
  for (let i = 0; i < 2; i++) {
    const k = i * 3;
    objs.push({
      read() {
        return k;
      },
      get val() {
        return k + 1;
      },
    });
  }
  console.log("t19", objs[0].read(), objs[1].read(), objs[0].val, objs[1].val);
}

// 具名函数表达式捕获体内 const（与 funcEnv 帧交互）
{
  const fns = [];
  for (let i = 0; i < 2; i++) {
    const k = i + 50;
    fns.push(function self() {
      return k;
    });
  }
  console.log("t20", fns[0](), fns[1]());
}

// 生成器内循环体 const 捕获
function* g21() {
  const fns = [];
  for (let i = 0; i < 2; i++) {
    const k = i + 30;
    fns.push(() => k);
    yield i;
  }
  yield fns.map((f) => f()).join(",");
}

// 异步函数内循环体 const 捕获（跨 await 保持按迭代实例）
async function t22() {
  const fns = [];
  const label = "A";
  for (let i = 0; i < 3; i++) {
    const k = i;
    await Promise.resolve();
    fns.push(() => label + k);
  }
  console.log("t22", fns.map((f) => f()).join(","));
}

{
  const it = g21();
  it.next();
  it.next();
  console.log("t21", it.next().value);
}
t22();
