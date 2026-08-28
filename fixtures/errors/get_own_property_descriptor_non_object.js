// Object.getOwnPropertyDescriptor 步骤 1 ToObject：nullish 参数抛 TypeError
// （基元如 42 会被包装为对象并返回 undefined，不再抛错）。
Object.getOwnPropertyDescriptor(null, 'x');
