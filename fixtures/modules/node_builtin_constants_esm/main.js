import constants, { F_OK, R_OK, W_OK, X_OK, SIGINT, SIGTERM, SIGKILL, COPYFILE_EXCL } from 'node:constants';
import constantsBare from 'constants';
console.log(constants === constantsBare);
console.log(F_OK, R_OK, W_OK, X_OK);
console.log(SIGINT, SIGTERM, SIGKILL);
console.log(COPYFILE_EXCL);
console.log(constants.F_OK === F_OK, constants.SIGINT === SIGINT);
console.log(typeof constants.ENOENT);
