// Array.prototype.slice.call 必须支持真数组与 array-like（arguments）。
// 旧路径：arr_proto 无 Function.prototype.call；slice 只接受真数组；
// 嵌套 host 重入时 WasmEnv::from_caller 失败。

const a = [1, 2, 3, 4];
console.log(a.slice(1, 3).join(","));
console.log(Array.prototype.slice.call(a, 1, 3).join(","));

function fromArgs() {
  return Array.prototype.slice.call(arguments);
}
console.log(fromArgs(9, 8, 7).join(","));

function fromEmptySlice() {
  return [].slice.call(arguments);
}
console.log(fromEmptySlice(1, 2).join(","));

function likeFromArgs() {
  return Array.prototype.slice.call(arguments, 1);
}
console.log(likeFromArgs("skip", "y", "z").join(","));

console.log(Array.prototype.slice.length, Array.prototype.slice.name);
console.log(Function.prototype.call.length, Function.prototype.call.name);
