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
