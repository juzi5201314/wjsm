// ES §10.2.11 步骤 22.a：严格模式**或**非 simple parameter list 一律建 unmapped
// arguments 对象——没有 [[ParameterMap]]，形参与索引属性互不影响，callee 抛错。

function calleeKind(args) {
  try {
    args.callee;
    return "no-throw";
  } catch (error) {
    return error.constructor.name;
  }
}

// 默认值形参
function withDefault(a = 0) {
  a = 5;
  const afterParamWrite = arguments[0];
  arguments[0] = 9;
  return [afterParamWrite, a, calleeKind(arguments)].join(",");
}
console.log(withDefault(1));

// rest 形参：rest 本身不占索引，前面的具名形参也不映射。
function withRest(a, ...rest) {
  a = 5;
  arguments[1] = 9;
  return [arguments[0], rest.join("|"), calleeKind(arguments)].join(",");
}
console.log(withRest(1, 2));

// 解构形参
function withPattern({ x }) {
  const first = arguments[0].x;
  arguments[0] = 9;
  return [first, x, calleeKind(arguments)].join(",");
}
console.log(withPattern({ x: 3 }));

// 严格模式 + 简单形参列表同样 unmapped。
function strictSimple(a) {
  "use strict";
  a = 5;
  return [arguments[0], calleeKind(arguments)].join(",");
}
console.log(strictSimple(1));

// 零形参是简单形参列表：仍是 mapped 对象（callee 为数据属性），只是没有可映射下标。
function noParams() {
  return [arguments.length, calleeKind(arguments), arguments.callee === noParams].join(",");
}
console.log(noParams(1, 2));

// 箭头函数没有自己的 arguments，取到的是外层函数的那个。
function arrowSeesOuter(a) {
  const read = () => arguments[0];
  a = 7;
  return read();
}
console.log(arrowSeesOuter(1));

// 非简单形参列表的 generator / async 同样 unmapped，且跨 suspend 保持独立。
function* genDefault(a = 0) {
  a = 5;
  yield arguments[0];
  arguments[0] = 9;
  yield a;
}
console.log([...genDefault(1)].join(","));

async function asyncRest(a, ...rest) {
  await null;
  a = 5;
  return [arguments[0], calleeKind(arguments)].join(",");
}
asyncRest(1).then((value) => console.log(value));
