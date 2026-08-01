// 回归锁定：循环体内纯调用（is_exception 恒 false → 死块折叠）且循环出口
// 块位于回边块之前。折叠 `idx = false_idx`（回边块）若跳过出口块，循环后的
// console.log 会静默丢失（输出为空而非 3）。
function pure() {}
let i = 0;
while (i < 3) {
  pure();
  i++;
}
console.log(i);
