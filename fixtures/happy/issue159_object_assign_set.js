// #159 Object.assign 走 [[Set]] 语义：对只读目标属性抛 TypeError；触发目标 setter
const target = {};
Object.defineProperty(target, "ro", { value: 0, writable: false, enumerable: true });
let threw = false;
let isTypeError = false;
try {
  Object.assign(target, { ro: 1 });
} catch (e) {
  threw = true;
  isTypeError = e instanceof TypeError;
}
console.log("threw:", threw, isTypeError, "ro:", target.ro);

let setterCalls = 0;
const t2 = {};
Object.defineProperty(t2, "p", {
  get() {
    return 0;
  },
  set() {
    setterCalls++;
  },
  enumerable: true,
});
Object.assign(t2, { p: 9 });
console.log("setterCalls:", setterCalls);
