// arguments 对象的自有属性形状（ES §10.4.4 CreateMappedArgumentsObject 步骤 17-21
// 与 §10.4.4 CreateUnmappedArgumentsObject 步骤 8-9）：索引可枚举，length 与
// callee 不可枚举，@@iterator 不可枚举，unmapped 的 callee 是抛错访问器。

function descriptors(a) {
  const index = Object.getOwnPropertyDescriptor(arguments, "0");
  const length = Object.getOwnPropertyDescriptor(arguments, "length");
  const callee = Object.getOwnPropertyDescriptor(arguments, "callee");
  const iterator = Object.getOwnPropertyDescriptor(arguments, Symbol.iterator);
  return [
    [index.writable, index.enumerable, index.configurable].join(""),
    [length.value, length.writable, length.enumerable, length.configurable].join(""),
    [callee.writable, callee.enumerable, callee.configurable, "get" in callee].join(""),
    [iterator.writable, iterator.enumerable, iterator.configurable].join(""),
  ].join(" ");
}
console.log(descriptors(1, 2));

// callee 不可枚举 ⇒ 不出现在 keys / for-in / 展开 / JSON / Object.values 里。
function enumeration(a) {
  a = 8;
  const forIn = [];
  for (const key in arguments) {
    forIn.push(key);
  }
  return [
    Object.keys(arguments).join(","),
    forIn.join(","),
    JSON.stringify({ ...arguments }),
    JSON.stringify(arguments),
    Object.values(arguments).join(","),
    Object.entries(arguments).map((entry) => entry.join(":")).join(","),
  ].join(" ");
}
console.log(enumeration(1, 2));

// 自有键顺序：索引升序 → length → callee（符号键不进 getOwnPropertyNames）。
function ownNames(a) {
  return [
    Object.getOwnPropertyNames(arguments).join(","),
    Object.getOwnPropertySymbols(arguments).map(String).join(","),
  ].join(" ");
}
console.log(ownNames(1, 2));
console.log(ownNames());

// mapped 的 callee 是数据属性，可写可配置；能被重写与删除。
function mutableCallee(a) {
  arguments.callee = 1;
  const afterWrite = arguments.callee;
  const deleted = delete arguments.callee;
  return [afterWrite, deleted, "callee" in arguments].join(",");
}
console.log(mutableCallee(1));

// unmapped（非简单形参列表）的 callee 是同一个 %ThrowTypeError%，读写都抛。
function unmappedCallee(a = 0) {
  const descriptor = Object.getOwnPropertyDescriptor(arguments, "callee");
  const results = [
    descriptor.get === descriptor.set,
    descriptor.enumerable,
    descriptor.configurable,
  ];
  try {
    arguments.callee;
    results.push("no-throw");
  } catch (error) {
    results.push(error.constructor.name);
  }
  // 直接调用 [[Set]] 也抛（§10.4.4 步骤 8 的 %ThrowTypeError% 同时占 get/set）。
  try {
    descriptor.set.call(arguments, 1);
    results.push("no-throw");
  } catch (error) {
    results.push(error.constructor.name);
  }
  return results.join(",");
}
console.log(unmappedCallee(1));

// arguments 是普通 Object 分支的 exotic 对象：不是数组，原型是 Object.prototype。
function shape(a) {
  return [
    Array.isArray(arguments),
    Object.getPrototypeOf(arguments) === Object.prototype,
    Object.prototype.toString.call(arguments),
    typeof arguments[Symbol.iterator],
  ].join(",");
}
console.log(shape(1));
