// §13.3 早期错误：OptionalChain 不含 TemplateLiteral 产生式，
// 标签模板作用于可选链是 SyntaxError（括号包裹的独立链不受限）。
const target = { tag: (parts) => parts[0] };
console.log(target?.tag`hello`);
