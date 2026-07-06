const fs = require('fs');
const fsp = require('fs/promises');
const missing = '/tmp/wjsm_issue_308_missing_' + process.pid;
try { fs.readFileSync(missing); } catch (e) { console.log('sync', e.code, e.syscall); }
fsp.readFile(missing).catch((e) => console.log('promise', e.code, e.syscall));
