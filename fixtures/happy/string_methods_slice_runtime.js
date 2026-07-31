// 运行时字符串原始方法解析：slice/concat（被直连优化排除的方法走原生 callable 回退）。
const s = 'hello world';
console.log(s.slice(1, 5));
console.log(s.slice(-5));
const h = 'hello';
console.log(h.concat(' ', 'world'));
console.log(typeof s.slice);
