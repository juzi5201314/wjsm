const http = require('http');
const https = require('node:https');
console.log(typeof http.request, typeof http.get, typeof https.get);

try {
  http.createServer().listen(0);
  console.log('unexpected');
} catch (error) {
  console.log(error.message.includes('issue #313'));
}
