// 循环导入窗口的命名空间语义（§10.4.6.8）：命名空间对象在任何模块体执行前
// 已物化（Link 阶段），先执行的 b 读取 a 的未初始化 let/const 导出必须抛
// ReferenceError（TDZ），而非缺属性 undefined；[[GetOwnProperty]] 按 V8 口径
// 抛 "{key} is not defined"；Object.keys 逐键 [[GetOwnProperty]] 同样抛出；
// exotic 身份（null 原型 / 不可扩展 / @@toStringTag / [[Set]] 拒绝）在窗口内
// 即已生效。
import * as b from './b.js';
export const fromA = 'a';
console.log('inA', b.fromB);
