const http = require('http');
const https = require('node:https');
console.log(typeof http.request, typeof http.get, typeof https.get);

var server = http.createServer((req, res) => {
  res.end('ok');
});

server.listen(0, '127.0.0.1', () => {
  console.log('listening', server.address().port > 0);
  server.close();
  console.log('closed');
  process.exit(0);
});
