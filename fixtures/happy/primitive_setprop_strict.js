"use strict";
// strict 模式下对基元接收者的属性/下标写入抛 TypeError（PutValue 步骤 6.c：
// succeeded 为 false 且 strict 为 true）。字符串奇异对象的 in-range 下标与
// length 是自有不可写数据属性，错误措辞区分 read only 与 create（与 V8 一致）。
var s = "hello";
try {
  s.x = 1;
} catch (error) {
  console.log("str-prop", error.name, "|", error.message);
}
try {
  s[0] = "H";
} catch (error) {
  console.log("str-elem", error.name, "|", error.message);
}
try {
  s.length = 3;
} catch (error) {
  console.log("str-length", error.name, "|", error.message);
}
try {
  s[99] = 1;
} catch (error) {
  console.log("str-oob", error.name, "|", error.message);
}

var n = 5;
try {
  n.x = 1;
} catch (error) {
  console.log("num-prop", error.name, "|", error.message);
}

var b = true;
try {
  b.x = 1;
} catch (error) {
  console.log("bool-prop", error.name, "|", error.message);
}

var sym = Symbol("k");
try {
  sym.x = 1;
} catch (error) {
  console.log("sym-prop", error.name, "|", error.message);
}

var big = 7n;
try {
  big.x = 1;
} catch (error) {
  console.log("bigint-prop", error.name, "|", error.message);
}

// 长堆字符串（超出 SSO 内联上限）走同一路径。
var heap = "this is a long heap string beyond sso limit for sure";
try {
  heap[2] = "x";
} catch (error) {
  console.log("heap-elem", error.name, "|", error.message);
}

// null / undefined base 与 sloppy 相同：TypeError。
try {
  null.x = 1;
} catch (error) {
  console.log("null-prop", error.name, "|", error.message);
}

// 基元值本身未被改变（BigInt 经 String() 归一，避免依赖 console 渲染差异）。
console.log("unchanged", s, n, b, typeof sym, String(big));
