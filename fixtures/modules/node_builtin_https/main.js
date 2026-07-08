const https = require('node:https');
console.log(typeof https.createServer, typeof https.request, typeof https.get);

const host = globalThis.__wjsm_node_tls;
host.serverListen(0, '127.0.0.1', '', '', '').then(function (server) {
  console.log('listening', host.serverPort(server) > 0);
  host.serverClose(server).then(function () {
    console.log('closed');
    process.exit(0);
  });
});
