import consoleDefault, { Console, log } from 'node:console';
import consoleBare from 'console';
console.log(consoleDefault === consoleBare);
console.log(consoleDefault === console);
log('named log', 'fmt', 7);
const custom = new Console(process.stdout, process.stderr);
custom.log('custom stdout line');
custom.error('custom stderr line');
const single = new Console({ stdout: process.stdout });
single.warn('warn falls back to stdout');
let threw = false;
try {
  new Console();
} catch (e) {
  threw = e instanceof TypeError;
}
console.log(threw);
// Console 方法保留全部实参（含 undefined），不截到 6 项。
custom.log(1, 2, 3, 4, 5, 6, 7, 8);
custom.log('m %s %d', 'a', 2, 'extra', undefined, null, 9, 'tail');
custom.info('i|%s', undefined);
custom.warn('w', undefined, 3, 4, 5, 6, 7, 8);
custom.error('e %d %s', 1, 2, 3, 4, 5, 6, 7, 8);
