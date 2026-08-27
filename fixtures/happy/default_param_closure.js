// 默认形参值为闭包/箭头时，表达式延续（expr_merge_block）不得泄漏给
// 调用方的 store 解析：回归 lower_default_value_check 的延续消耗与 phi 前驱。
function plain(a, cb = function () { return 42; }) {
  return cb();
}
console.log(plain(5));
console.log(plain(5, function () { return 7; }));

// generator body：默认闭包与经续体槽传入的 arguments 对象共存。
function* gen(a, cb = function () { return arguments.length; }) {
  yield arguments.length;
  yield cb(9, 8);
}
const gi = gen(7);
console.log(gi.next().value);
console.log(gi.next().value);

// 箭头默认值捕获同函数的前序形参。
function* genArrow(a, cb = () => a * 2) {
  yield cb();
}
console.log(genArrow(21).next().value);

// async body：默认闭包 + rest 形参经续体槽传入。
async function asyncMixed(a = function () { return 1; }, ...r) {
  return a() + r.length;
}

// async generator body：默认闭包自身的 arguments 独立于外层。
async function* asyncGen(x = function () { return arguments.length; }) {
  yield x(1, 2, 3);
}

(async () => {
  console.log(await asyncMixed(undefined, 2, 3));
  console.log((await asyncGen().next()).value);
})();
