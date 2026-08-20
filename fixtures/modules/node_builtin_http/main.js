const http = require('http');
const https = require('node:https');
console.log(typeof http.request, typeof http.get, typeof https.get);
