const tls = require('node:tls');
console.log(typeof tls.createServer, typeof tls.connect, typeof tls.TLSSocket);

const host = globalThis.__wjsm_node_tls;
host.serverListen(0, '127.0.0.1', '', '', '').then(function (server) {
  const port = host.serverPort(server);
  console.log('listening', port > 0);
  host.serverAccept(server).then(function (socket) {
    host.write(socket, 'hello tls');
    host.end(socket);
  });
  host.connect(port, '127.0.0.1', 'localhost', false, '').then(function (client) {
    console.log('connected');
    host.read(client).then(function (data) {
      console.log('received:', data !== null);
      host.destroy(client);
      host.serverClose(server).then(function () {
        console.log('closed');
        process.exit(0);
      });
    });
  }, function (error) {
    console.log('error:', error.message);
    process.exit(1);
  });
});
