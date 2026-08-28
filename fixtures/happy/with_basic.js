// with 基础语义：对象属性读/写、词法遮蔽、嵌套 with、原型链解析、var 提升。
const o = { x: 1, y: 2 };
with (o) {
  console.log(x, y);
  x = 10;
  var z = x + y;
}
console.log(o.x, o.y, z, "z" in o);

let a = "outer";
with ({ a: "inner" }) {
  console.log(a);
  a = "written";
}
console.log(a, "a" in o);

with ({ b: 1 }) {
  with ({ c: 2 }) {
    console.log(b + c);
    b = 100;
    c = 200;
  }
}

const proto = { p: "from-proto" };
const child = Object.create(proto);
with (child) {
  console.log(p);
  p = "own";
}
console.log(child.p, proto.p, Object.prototype.hasOwnProperty.call(child, "p"));

var hoisted = "outer";
const src = { hoisted: "in-with" };
with (src) {
  var hoisted;
  console.log(hoisted);
  hoisted = "written";
}
console.log(hoisted, src.hoisted);
