function getHost() {
  const host = globalThis.__wjsm_node_fs;
  if (!host) throw new Error('wjsm internal fs host bridge is not installed');
  return host;
}
const host = getHost();

export const constants = {
  F_OK: 0,
  R_OK: 4,
  W_OK: 2,
  X_OK: 1,
  COPYFILE_EXCL: 1,
  COPYFILE_FICLONE: 2,
  COPYFILE_FICLONE_FORCE: 4,
};



export class Stats {
  constructor(raw) {
    this.size = raw.size;
    this.mode = raw.mode;
    this.mtimeMs = raw.mtimeMs;
    this.atimeMs = raw.atimeMs;
    this.ctimeMs = raw.ctimeMs;
    this.birthtimeMs = raw.birthtimeMs;
    this.mtime = new Date(this.mtimeMs);
    this.atime = new Date(this.atimeMs);
    this.ctime = new Date(this.ctimeMs);
    this.birthtime = new Date(this.birthtimeMs);
    this.__kind = raw.kind;
  }
  isFile() { return this.__kind == 'file'; }
  isDirectory() { return this.__kind == 'directory'; }
  isSymbolicLink() { return this.__kind == 'symlink'; }
  isBlockDevice() { return this.__kind == 'block'; }
  isCharacterDevice() { return this.__kind == 'character'; }
  isFIFO() { return this.__kind == 'fifo'; }
  isSocket() { return this.__kind == 'socket'; }
}

class Dirent {
  constructor(raw) {
    this.name = raw.name;
    this.__kind = raw.kind;
  }
  isFile() { return this.__kind == 'file'; }
  isDirectory() { return this.__kind == 'directory'; }
  isSymbolicLink() { return this.__kind == 'symlink'; }
  isBlockDevice() { return this.__kind == 'block'; }
  isCharacterDevice() { return this.__kind == 'character'; }
  isFIFO() { return this.__kind == 'fifo'; }
  isSocket() { return this.__kind == 'socket'; }
}

export function readFileSync(path, options) { return host.readFileSync(path, options); }
export function writeFileSync(path, data, options) { return host.writeFileSync(path, data, options); }
export function appendFileSync(path, data, options) { return host.appendFileSync(path, data, options); }
export function existsSync(path) { const value = host.existsSync(path); return typeof value === 'boolean' ? value : 0 > 1; }
export function statSync(path) { return new Stats(host.statSync(path)); }
export function lstatSync(path) { return new Stats(host.lstatSync(path)); }
export function readdirSync(path, options) {
  const withFileTypes = Boolean(options && options.withFileTypes);
  const rawEntries = host.readdirSync(path, withFileTypes);
  if (!withFileTypes) return rawEntries;
  const entries = [];
  for (let i = 0; i < rawEntries.length; i++) {
    entries.push(new Dirent(rawEntries[i]));
  }
  return entries;
}
export function mkdirSync(path, options) { return host.mkdirSync(path, options); }
export function rmSync(path, options) { return host.rmSync(path, options); }
export function unlinkSync(path) { return host.unlinkSync(path); }
export function renameSync(oldPath, newPath) { return host.renameSync(oldPath, newPath); }
export function copyFileSync(src, dest, mode) { return host.copyFileSync(src, dest, mode); }
export function accessSync(path, mode) { return host.accessSync(path, mode); }
export function realpathSync(path) { return host.realpathSync(path); }
export function readlinkSync(path) { return host.readlinkSync(path); }
export function symlinkSync(target, path, type) { return host.symlinkSync(target, path, type); }
export function chmodSync(path, mode) { return host.chmodSync(path, mode); }
export function chownSync(path, uid, gid) { return host.chownSync(path, uid, gid); }

const fs = {
  readFileSync,
  writeFileSync,
  appendFileSync,
  copyFileSync,
  accessSync,
  renameSync,
  existsSync,
  statSync,
  lstatSync,
  readdirSync,
  mkdirSync,
  rmSync,
  unlinkSync,
  chmodSync,
  symlinkSync,
  readlinkSync,
  realpathSync,
  chownSync,
  Stats,
  constants,
};
export default fs;
