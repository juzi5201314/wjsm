// 类名绑定按每次求值新建 classEnv（ClassDefinitionEvaluation 步骤 3）：
// 循环内类表达式 / 类声明的方法闭包捕获各轮独立的类对象与原型，不共享槽位。
const pairs = [];
for (let i = 0; i < 3; i++) {
  const K = class C { tag() { return C; } };
  pairs.push([K, new K()]);
}
console.log(pairs[0][1].tag() === pairs[0][0]);
console.log(pairs[1][1].tag() === pairs[1][0]);
console.log(pairs[0][1].tag() === pairs[2][0]);

const decls = [];
for (const n of [10, 20]) {
  class D {
    static off = 100;
    base() { return D.off + n; }
  }
  decls.push(D);
}
console.log(decls[0] === decls[1]);
console.log(new decls[0]().base(), new decls[1]().base());
console.log(decls[0].prototype === decls[1].prototype);
