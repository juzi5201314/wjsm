import proc, { platform, versions, cwd, nextTick } from 'node:process';
import procBare from 'process';
console.log(proc === procBare);
console.log(proc === process, proc === globalThis.process);
console.log(platform === process.platform, typeof platform);
console.log(typeof versions.node);
console.log(typeof cwd(), cwd() === process.cwd());
nextTick(() => console.log('nextTick ran'));
console.log('sync end');
