// process.stdin flowing 模式（管道输入）：注册 'data' 即开始流动，
// Buffer 块交付后依次发 'end'（readable 翻 false）与 'close'。
// 输出与 Node v22 管道运行逐字节一致（oracle 校验）。
console.log('sync', process.stdin.fd, process.stdin.isTTY, typeof process.stdin.read);
console.log('paused-before', process.stdin.isPaused());

process.stdin.once('data', (chunk) => {
  console.log('once-data', Buffer.isBuffer(chunk), JSON.stringify(chunk.toString()));
});
process.stdin.on('data', (chunk) => {
  console.log('data', chunk.length, JSON.stringify(chunk.toString()));
});
process.stdin.on('end', () => console.log('end', process.stdin.readable));
process.stdin.on('close', () => console.log('close'));
