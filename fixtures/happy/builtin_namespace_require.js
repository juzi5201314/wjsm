// CJS require 路径触发同一 builtin 间 `import * as` 链接：require('readline')
// 加载含内部命名空间导入的 builtin 不再触发 InternalInvariant，且经命名空间
// 拼装的导出与直接 require 子模块取到同一函数。
const readline = require('node:readline');
const promises = require('node:readline/promises');

console.log('promises typeof:', typeof readline.promises.createInterface);
console.log('fn identity:', readline.promises.createInterface === promises.createInterface);
console.log('interface identity:', readline.promises.Interface === promises.Interface);
