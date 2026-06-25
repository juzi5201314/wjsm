// #161 Object.create(非对象非 null 原型) 必须抛 TypeError（可被 JS 捕获）
let caught = "none";
try {
  Object.create(42);
  console.log("unexpected success");
} catch (e) {
  caught = e instanceof TypeError ? "TypeError" : e.name;
}
console.log(caught);
