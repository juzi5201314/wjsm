// process.stdin.setEncoding('utf8')：'data' 交付字符串（含多字节字符），
// 非法编码抛 TypeError。输出与 Node v22 管道运行逐字节一致（oracle 校验）。
try {
  process.stdin.setEncoding('koi8-r');
} catch (error) {
  console.log(error.name, error.message);
}
console.log('set', process.stdin.setEncoding('utf8') === process.stdin);

process.stdin.on('data', (chunk) => {
  console.log('data', typeof chunk, JSON.stringify(chunk));
});
process.stdin.on('end', () => console.log('end'));

// 异步迭代与 'data' 监听互斥使用；本 fixture 仅验证事件路径。
