const https = require('node:https');
console.log(typeof https.createServer, typeof https.request, typeof https.get);
