// process.stdin paused 模式：read() 在数据就绪前返回 null，'readable'
// 通知后按 read(size) 字节切读，EOF 前补发一次 'readable' 再发 'end'/'close'。
// 输出与 Node v22 管道运行逐字节一致（oracle 校验）。
console.log('first', process.stdin.read());

process.stdin.on('readable', () => {
  console.log('readable');
  let chunk;
  while ((chunk = process.stdin.read(3)) !== null) {
    console.log('read3', JSON.stringify(chunk.toString()));
  }
});
process.stdin.on('end', () => console.log('end'));
process.stdin.on('close', () => console.log('close'));
