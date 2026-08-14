// typed math thunk 的守卫：实参未证明 f64 时保留 dispatcher 的
// ToNumber 强制转换与 BigInt TypeError 语义。
function check(label, fn) {
  try {
    const value = fn();
    console.log(label, value);
  } catch (e) {
    console.log(label, e.name);
  }
}

check("sin(string)", function () { return Math.sin("0.5"); });
check("sin(bigint)", function () { return Math.sin(1n); });
check("atan2(string,bigint)", function () { return Math.atan2("1", 2n); });
check("pow(bigint,number)", function () { return Math.pow(2n, 3); });
check("pow(symbol,number)", function () { return Math.pow(Symbol("x"), 3); });
