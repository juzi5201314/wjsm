// Node 弃用的 constants 模块：把 os.constants 与 fs.constants 摊平到顶层。
import { constants as osConstants } from 'node:os';
import { constants as fsConstants } from 'node:fs';

const constants = {};
Object.assign(constants, osConstants.errno);
Object.assign(constants, osConstants.signals);
Object.assign(constants, fsConstants);

export const ENOENT = osConstants.errno.ENOENT;
export const EACCES = osConstants.errno.EACCES;
export const EEXIST = osConstants.errno.EEXIST;
export const SIGINT = osConstants.signals.SIGINT;
export const SIGTERM = osConstants.signals.SIGTERM;
export const SIGKILL = osConstants.signals.SIGKILL;
export const F_OK = fsConstants.F_OK;
export const R_OK = fsConstants.R_OK;
export const W_OK = fsConstants.W_OK;
export const X_OK = fsConstants.X_OK;
export const COPYFILE_EXCL = fsConstants.COPYFILE_EXCL;
export const COPYFILE_FICLONE = fsConstants.COPYFILE_FICLONE;
export const COPYFILE_FICLONE_FORCE = fsConstants.COPYFILE_FICLONE_FORCE;

export default constants;
