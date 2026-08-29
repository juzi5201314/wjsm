// builtin 之间 `import * as ns`（readline → readline/promises）曾触发模块
// 链接层 InternalInvariant：拆分 image 下 $builtin_main 的命名空间注册以
// (builtin image, ModuleId) 查键落空。本 fixture 锁定修复后的行为：
// 静态 `import * as`、运行时动态 import()、builtin 内部经命名空间拼装的
// 导出共享同一 canonical 对象（§10.4.6.12 GetModuleNamespace 缓存）。
import * as promisesNs from 'node:readline/promises';
import readline from 'node:readline';

console.log('promises keys:', Object.keys(readline.promises).join(','));
console.log('fn identity:', readline.promises.createInterface === promisesNs.createInterface);
console.log('ns tag:', Object.prototype.toString.call(promisesNs));

const spec = 'node:readline' + '/promises';
import(spec).then((dynamicNs) => {
  console.log('ns identity:', dynamicNs === promisesNs);
  console.log('dynamic keys:', Object.keys(dynamicNs).join(','));
});
