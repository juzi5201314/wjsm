const childProcess = require('child_process');
const nodeChildProcess = require('node:child_process');
console.log(childProcess === nodeChildProcess);
console.log(typeof childProcess.spawnSync, typeof childProcess.execSync, typeof childProcess.spawn);
try {
  childProcess.spawnSync('echo', ['ok']);
  console.log('unexpected');
} catch (error) {
  console.log(error.message.includes('child_process execution is disabled'));
}
