// Object.defineProperty 对函数添加超过初始容量(8)的属性、触发属性对象扩容后，
// 属性必须存活。函数属性对象 handle 经 __function_props_base 重定位；define_property
// host 扩容写槽若用裸函数表索引会写错 obj_table 槽 → 扩容后读到旧对象 → 丢属性。
function f(x) {
  return x + 1;
}
Object.defineProperty(f, "aa", { value: 1, configurable: true });
Object.defineProperty(f, "bb", { value: 2, configurable: true });
Object.defineProperty(f, "cc", { value: 3, configurable: true });
Object.defineProperty(f, "dd", { value: 4, configurable: true });
Object.defineProperty(f, "ee", { value: 5, configurable: true });
Object.defineProperty(f, "ff", { value: 6, configurable: true });
Object.defineProperty(f, "gg", { value: 7, configurable: true });
Object.defineProperty(f, "hh", { value: 8, configurable: true });
Object.defineProperty(f, "ii", { value: 9, configurable: true });
Object.defineProperty(f, "jj", { value: 10, configurable: true });
// 扩容前后的属性都要在；函数本身仍可调用、.name 完好。
console.log(f.aa, f.ff, f.gg, f.jj);
console.log(f.name, f(41));
