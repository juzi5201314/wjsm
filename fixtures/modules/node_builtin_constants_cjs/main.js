const constants = require('constants');
console.log(constants === require('node:constants'));
console.log(constants.F_OK, constants.R_OK, constants.W_OK, constants.X_OK);
console.log(constants.SIGINT, constants.SIGTERM, constants.SIGKILL);
console.log(typeof constants.ENOENT === 'number');
