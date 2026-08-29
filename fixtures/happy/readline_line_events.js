// node:readline createInterface({ input: process.stdin })：'line' 按
// \r\n / \n / 孤立 \r 拆分，EOF 交付残余行并发 'close'。
// 输出与 Node v22 管道运行逐字节一致（oracle 校验）。
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, prompt: 'P> ' });
console.log('terminal', rl.terminal, 'prompt', JSON.stringify(rl.getPrompt()));
rl.setPrompt('Q> ');
console.log('prompt-after-set', JSON.stringify(rl.getPrompt()));

rl.on('line', (line) => console.log('line', JSON.stringify(line)));
rl.on('close', () => console.log('close'));
