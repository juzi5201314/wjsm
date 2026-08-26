// 6 码元 ASCII SSO 经数组索引/shift 读取必须保持完整 payload。
const direct = ["abcdef", " world"];
console.log(direct[0], direct[1]);
const shifted = ["hello", " world"];
console.log(shifted.shift(), shifted.shift());
