// 宿主 stdin 分块交付（.stdin-chunk 侧文件 → WJSM_TEST_STDIN_CHUNK=3）：
// 多字节字符与 \r\n 被劈到不同块，readline 跨块拼接后行输出必须与
// Node v22 单块管道运行逐字节一致（oracle 校验）。
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => console.log('line', JSON.stringify(line)));
rl.on('close', () => console.log('close'));
