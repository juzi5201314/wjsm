// 变量重绑定后读取属性不得折叠回旧字面量初值（object_literal_read_fold 残留绑定回归）。
let v = { x: 1 };
v = 5;
console.log(v.x);

let w = { x: 7 };
const alias = w;
w = { x: 8 };
console.log(alias.x, w.x);
