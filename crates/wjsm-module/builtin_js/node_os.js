function getHost() {
  const host = globalThis.__wjsm_node_os;
  if (!host) throw new Error('wjsm internal os host bridge is not installed');
  return host;
}
const platformValue = process.platform;
const archValue = process.arch;


export function platform() { return platformValue; }
export function arch() { return archValue; }
export function tmpdir() { return getHost().tmpdir(); }
export function homedir() { return getHost().homedir(); }
export function hostname() { return getHost().hostname(); }
export function cpus() { return getHost().cpus(); }
export function totalmem() { return getHost().totalmem(); }
export function freemem() { return getHost().freemem(); }
export function type() { return getHost().type(); }
export function release() { return getHost().release(); }
export function version() { return getHost().version(); }
export function networkInterfaces() { return getHost().networkInterfaces(); }
export const EOL = platformValue === 'win32' ? '\r\n' : '\n';
export const constants = {
  signals: { SIGINT: 2, SIGTERM: 15, SIGKILL: 9 },
  errno: { ENOENT: -2, EACCES: -13, EEXIST: -17 }
};
const os = {
  platform,
  arch,
  tmpdir,
  homedir,
  hostname,
  cpus,
  totalmem,
  freemem,
  type,
  release,
  version,
  networkInterfaces,
  EOL,
  constants
};
export default os;
