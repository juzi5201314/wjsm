const host = globalThis.__wjsm_node_fs;

export function readFile(path, options) {
  return new Promise((resolve, reject) => {
    try { const value = host.readFileSync(path, options); resolve(value); } catch (error) { reject(error); }
  });
}
export function writeFile(path, data, options) {
  return new Promise((resolve, reject) => {
    try { const value = host.writeFileSync(path, data, options); resolve(value); } catch (error) { reject(error); }
  });
}
export function stat(path) {
  return new Promise((resolve, reject) => {
    try { const value = host.statSync(path); resolve(value); } catch (error) { reject(error); }
  });
}
export function readdir(path, options) {
  return new Promise((resolve, reject) => {
    try { const value = host.readdirSync(path, Boolean(options && options.withFileTypes)); resolve(value); } catch (error) { reject(error); }
  });
}
export function mkdir(path, options) {
  return new Promise((resolve, reject) => {
    try { const value = host.mkdirSync(path, options); resolve(value); } catch (error) { reject(error); }
  });
}
export function rm(path, options) {
  return new Promise((resolve, reject) => {
    try { const value = host.rmSync(path, options); resolve(value); } catch (error) { reject(error); }
  });
}
export function unlink(path) {
  return new Promise((resolve, reject) => {
    try { const value = host.unlinkSync(path); resolve(value); } catch (error) { reject(error); }
  });
}
export function rename(oldPath, newPath) {
  return new Promise((resolve, reject) => {
    try { const value = host.renameSync(oldPath, newPath); resolve(value); } catch (error) { reject(error); }
  });
}
export function copyFile(src, dest, mode) {
  return new Promise((resolve, reject) => {
    try { const value = host.copyFileSync(src, dest, mode); resolve(value); } catch (error) { reject(error); }
  });
}
export function access(path, mode) {
  return new Promise((resolve, reject) => {
    try { const value = host.accessSync(path, mode); resolve(value); } catch (error) { reject(error); }
  });
}
export function realpath(path) {
  return new Promise((resolve, reject) => {
    try { const value = host.realpathSync(path); resolve(value); } catch (error) { reject(error); }
  });
}
const promises = {
  readFile,
  writeFile,
  stat,
  readdir,
  mkdir,
  rm,
  unlink,
  rename,
  copyFile,
  access,
  realpath,
};
export default promises;
