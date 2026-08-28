// 静态字符串说明符的动态 import 指向 node: 内置模块（AOT 静态路径，
// 区别于 dynamic_import_builtin_runtime 的运行时拼接说明符路径）：
// promise 必须兑现带真实导出的命名空间，@@toStringTag 呈现为 "Module"。
import('node:url').then((ns) => {
  console.log(typeof ns.fileURLToPath, typeof ns.pathToFileURL, typeof ns.URL, typeof ns.default);
  console.log(ns.resolve('http://example.com/a/b', '../c'));
  console.log(Object.keys(ns).includes('fileURLToPath'), Object.keys(ns).includes('Symbol.toStringTag'));
  console.log(Object.prototype.toString.call(ns), ns[Symbol.toStringTag]);
  const tagDesc = Object.getOwnPropertyDescriptor(ns, Symbol.toStringTag);
  console.log(tagDesc.value, tagDesc.writable, tagDesc.enumerable, tagDesc.configurable);
  return import('node:querystring').then((qs) => {
    console.log(typeof qs.stringify, qs.stringify({ a: 1 }));
  });
});
