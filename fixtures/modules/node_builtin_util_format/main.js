import util, { format } from 'node:util';

// 参数全量保留：undefined 不被过滤，超过 6 个实参不截断。
console.log(format('a|%s', undefined));
console.log(format(1, undefined));
console.log(format('%s', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'));
console.log(format('x', 'y'));
console.log(format('m %s %d', 'a', 2, 'extra', undefined, null, 9, 'tail'));
console.log(format('a', undefined, 'b', undefined));

// 占位符边界：实参耗尽保留字面、未知指令不消耗、%% 转义、%c 消耗不输出。
console.log(format('%s %s', 'only'));
console.log(format('%x kept', 'extra'));
console.log(format('100%%', 'x'));
console.log(format('%c<', 'css-rule', 'after'));

// 数值占位符：-0、%i 截断、%f parseFloat 前缀解析。
console.log(format('%d|%i|%f', -0, 42.9, '42px'));
console.log(format('%j', undefined));

// %s 对对象走浅层 inspect，嵌套折叠为 [Object]/[Array]。
console.log(format('%s', { a: { b: 1 } }));
console.log(format('%s', [1, [2, 3]]));
console.log(format({ a: 1 }, 'b', 2));

// promisify/callbackify 同样保留全部实参与 undefined。
const promisified = util.promisify(function (a, b, c, d, e, f, g, cb) {
  cb(null, [a, b, c, d, e, f, g].map(String).join('|'));
});
const callbackified = util.callbackify(async function (...xs) {
  return xs.length + ':' + xs.map(String).join(',');
});
promisified(1, undefined, 3, 4, 5, 6, 7).then((v) => {
  console.log(v);
  callbackified(1, undefined, 3, 4, 5, 6, 7, (err, v2) => console.log(v2));
});
