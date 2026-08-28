const proc = globalThis.process;

export const env = proc.env;
export const argv = proc.argv;
export const execArgv = proc.execArgv;
export const execPath = proc.execPath;
export const platform = proc.platform;
export const arch = proc.arch;
export const version = proc.version;
export const versions = proc.versions;
export const pid = proc.pid;
export const ppid = proc.ppid;
export const stdin = proc.stdin;
export const stdout = proc.stdout;
export const stderr = proc.stderr;
export const cwd = proc.cwd;
export const exit = proc.exit;
export const nextTick = proc.nextTick;
export const on = proc.on;
export const hrtime = proc.hrtime;
export const uptime = proc.uptime;
export const memoryUsage = proc.memoryUsage;
export const cpuUsage = proc.cpuUsage;

export default proc;
