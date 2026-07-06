const fs = require('fs');
const dir = '/tmp/wjsm_issue_308_' + process.pid;
const nested = dir + '/nested';
const file = nested + '/file.txt';
const renamed = nested + '/renamed.txt';
const copied = nested + '/copied.txt';
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(nested, { recursive: true });
fs.writeFileSync(file, 'hello');
fs.appendFileSync(file, ' world');
console.log(fs.readFileSync(file, 'utf8'));
fs.renameSync(file, renamed);
fs.copyFileSync(renamed, copied);
fs.accessSync(copied, fs.constants.F_OK | fs.constants.R_OK);
fs.chmodSync(copied, 0o644);
console.log(fs.existsSync(renamed), fs.existsSync(copied), fs.statSync(copied).isFile());
if (process.platform !== 'win32') {
  const link = nested + '/link.txt';
  fs.symlinkSync(copied, link);
  console.log('symlink', fs.readlinkSync(link).includes('copied.txt'), fs.lstatSync(link).isSymbolicLink(), fs.realpathSync(link) === fs.realpathSync(copied));
}
fs.unlinkSync(renamed);
fs.rmSync(dir, { recursive: true, force: true });
console.log(fs.existsSync(dir));
