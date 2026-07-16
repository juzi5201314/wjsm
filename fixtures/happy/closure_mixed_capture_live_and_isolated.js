// 混合捕获：本层 binding 每次调用独立，外层 binding 保持 live。
// 旧语义要么把 local 写进父 env（多次调用互相覆盖），要么扁平复制外层值（丢失 live）。

function make() {
  let outer = 0;
  return function (x) {
    // 先创建仅外层捕获的闭包，再创建混合捕获——旧路径会把 $shared_env 钉成父 env。
    const readOuterOnly = () => outer;
    let local = x;
    return {
      readOuterOnly,
      readOuter: () => outer,
      writeOuter: (v) => {
        outer = v;
      },
      readLocal: () => local,
      writeLocal: (v) => {
        local = v;
      },
    };
  };
}

const factory = make();
const a = factory(1);
const b = factory(2);

a.writeOuter(10);
console.log(a.readOuter(), b.readOuter(), a.readOuterOnly(), b.readOuterOnly());

a.writeLocal(11);
console.log(a.readLocal(), b.readLocal());

// 三层嵌套：中间层也有自己的 local，外层写入仍应落到声明 frame。
function outer() {
  let shared = 1;
  return function mid(x) {
    const getSharedEarly = () => shared;
    let local = x;
    return {
      getSharedEarly,
      getShared: () => shared,
      setShared: (v) => {
        shared = v;
      },
      getLocal: () => local,
      setLocal: (v) => {
        local = v;
      },
    };
  };
}

const mid = outer();
const p1 = mid(10);
const p2 = mid(20);
p1.setShared(5);
console.log(p1.getShared(), p2.getShared(), p1.getSharedEarly(), p2.getSharedEarly());
p1.setLocal(11);
console.log(p1.getLocal(), p2.getLocal());
