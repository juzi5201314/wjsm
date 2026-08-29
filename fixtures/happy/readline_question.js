// readline question：查询写到 output、下一行被回调拦截（不触发 'line'），
// close 后 question 抛错。输出与 Node v22 管道运行逐字节一致（oracle 校验）。
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
rl.question('ask? ', (answer) => {
  console.log('got', JSON.stringify(answer));
  rl.close();
  try {
    rl.question('again? ', () => {});
  } catch (error) {
    console.log('after-close', error.name, error.message);
  }
});
rl.on('line', (line) => console.log('line-event', JSON.stringify(line)));
rl.on('close', () => console.log('closed'));
