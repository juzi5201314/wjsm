// for await (const chunk of process.stdin)：异步块迭代（管道路径）。
// 顶层 for await 需 ESM，oracle 按 CJS 解析 .js，故包一层 async IIFE。
// 输出与 Node v22 管道运行逐字节一致（oracle 校验）。
(async () => {
  const pieces = [];
  for await (const chunk of process.stdin) {
    pieces.push(chunk.toString());
  }
  console.log('chunks', JSON.stringify(pieces.join('')));
  console.log('done');
})();
