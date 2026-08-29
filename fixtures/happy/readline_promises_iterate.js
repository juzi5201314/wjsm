// node:readline/promises：question 返回 Promise，for await 迭代余下各行，
// EOF 关闭 interface 后迭代自然结束。
// 输出与 Node v22 管道运行逐字节一致（oracle 校验）。
import { createInterface } from 'node:readline/promises';

const rl = createInterface({ input: process.stdin, output: process.stdout });
const answer = await rl.question('name? ');
console.log('answer', JSON.stringify(answer));
for await (const line of rl) {
  console.log('iter', JSON.stringify(line));
}
console.log('done');
