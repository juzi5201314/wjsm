// 跨函数前向引用 TDZ：函数先于 let/const/class 声明执行时读取/写入绑定
// 应抛 ReferenceError；声明执行后同一函数正常读写。

// 读取：let
function readLet() {
  return letValue;
}
try {
  readLet();
  console.log("unreachable");
} catch (e) {
  console.log(e.constructor.name + ": " + e.message);
}
let letValue = 1;
console.log(readLet());

// 读取：const
function readConst() {
  return constValue;
}
try {
  readConst();
} catch (e) {
  console.log(e.constructor.name + ": " + e.message);
}
const constValue = 2;
console.log(readConst());

// 读取：class（构造）
function makeInstance() {
  return new Later();
}
try {
  makeInstance();
} catch (e) {
  console.log(e.constructor.name + ": " + e.message);
}
class Later {}
console.log(makeInstance() instanceof Later);

// 赋值 / 复合赋值 / 逻辑赋值 / update
function writeLet() {
  target = 5;
}
function addLet() {
  target += 1;
}
function orLet() {
  target ||= 9;
}
function bumpLet() {
  target++;
}
for (const early of [writeLet, addLet, orLet, bumpLet]) {
  try {
    early();
  } catch (e) {
    console.log(e.constructor.name + ": " + e.message);
  }
}
let target = 1;
writeLet();
addLet();
bumpLet();
orLet();
console.log(target);

// typeof 对 TDZ 绑定同样抛错（不同于未声明标识符）
function typeofLet() {
  return typeof tdzTypeof;
}
try {
  typeofLet();
} catch (e) {
  console.log(e.constructor.name + ": " + e.message);
}
let tdzTypeof = "ready";
console.log(typeofLet());

// 块级作用域
{
  const readBlock = () => blockValue;
  try {
    readBlock();
  } catch (e) {
    console.log(e.constructor.name + ": " + e.message);
  }
  let blockValue = 3;
  console.log(readBlock());
}

// 循环迭代环境：每次迭代绑定独立进入/退出 TDZ
function loopSection() {
  const fns = [];
  for (let i = 0; i < 2; i++) {
    const read = () => perIteration;
    if (i === 0) {
      try {
        read();
      } catch (e) {
        console.log(e.constructor.name + ": " + e.message);
      }
    }
    let perIteration = i * 10;
    fns.push(read);
  }
  console.log(fns[0](), fns[1]());
}
loopSection();

// 声明器自引用（初始化完成后调用方法读取自身）
const registry = {
  lookup() {
    return registry;
  },
};
console.log(registry.lookup() === registry);
const selfArrow = () => selfArrow;
console.log(selfArrow() === selfArrow);

// 嵌套函数：函数体内的前向引用
function outer() {
  function inner() {
    return innerValue;
  }
  try {
    inner();
  } catch (e) {
    console.log(e.constructor.name + ": " + e.message);
  }
  let innerValue = 7;
  return inner();
}
console.log(outer());
