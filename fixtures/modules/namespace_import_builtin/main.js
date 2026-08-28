// 静态 `import * as` 指向 node: 内置模块：命名空间必须兑现真实导出
// （builtin 段种子路径既往缺陷：builtin 来源未安装 getter，命名空间为空对象）。
// @@toStringTag 必须是真符号键的 "Module" 数据属性（§10.4.6.2）：
// 不可写/不可枚举/不可配置，且不得以字符串键混入 Object.keys。
import * as url from 'node:url';
import * as qs from 'node:querystring';

console.log(typeof url.fileURLToPath, typeof url.pathToFileURL, typeof url.URL, typeof url.default);
console.log(url.resolve('http://example.com/a/b', '../c'));
console.log(url.default.fileURLToPath === url.fileURLToPath);
console.log(Object.keys(url).includes('fileURLToPath'), Object.keys(url).includes('Symbol.toStringTag'));
console.log(Object.prototype.toString.call(url), url[Symbol.toStringTag]);
const tagDesc = Object.getOwnPropertyDescriptor(url, Symbol.toStringTag);
console.log(tagDesc.value, tagDesc.writable, tagDesc.enumerable, tagDesc.configurable);

console.log(typeof qs.stringify, typeof qs.parse);
console.log(qs.stringify({ a: 1, b: 'x y' }));
console.log(qs.parse('a=1&b=2').b);
console.log(Object.prototype.toString.call(qs));
