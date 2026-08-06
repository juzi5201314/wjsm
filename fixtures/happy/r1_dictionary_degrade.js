// delete 使对象退化为字典 shape：其余属性的值槽下标必须保持稳定，
// 否则已发射的访问点会读到错位的槽。
const o = { a: 1, b: 2, c: 3, d: 4 };
delete o.b;
console.log(o.a, o.b, o.c, o.d);
console.log(Object.keys(o).join(","), "b" in o, "c" in o);

// 字典 shape 上继续增删读写。
o.e = 5;
o.b = 20;
console.log(o.a, o.b, o.c, o.d, o.e);
console.log(Object.keys(o).join(","));
delete o.a;
delete o.e;
console.log(o.a, o.b, o.c, o.d, o.e, Object.keys(o).join(","));

// 删除不存在的属性；delete 返回值语义。
console.log(delete o.zzz, delete o.c, o.c);

// 退化只影响该对象，同源 shape 的其他对象不受影响。
const p = { a: 1, b: 2, c: 3, d: 4 };
const q = { a: 9, b: 8, c: 7, d: 6 };
delete p.c;
console.log(Object.keys(p).join(","), Object.keys(q).join(","));
console.log(q.a, q.b, q.c, q.d);

// 超过字典阈值（64 个属性）后全部可读写。
const many = {};
for (let i = 0; i < 70; i++) {
  many["k" + i] = i * 2;
}
console.log(many.k0, many.k33, many.k69, Object.keys(many).length);
delete many.k33;
console.log(many.k33, many.k0, many.k69, Object.keys(many).length);
for (let i = 0; i < 70; i++) {
  many["k" + i] = i + 1000;
}
console.log(many.k0, many.k33, many.k69, Object.keys(many).length);
