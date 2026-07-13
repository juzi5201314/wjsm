const { AsyncLocalStorage } = require('node:async_hooks');
const als = new AsyncLocalStorage();
const tlsHost = globalThis.__wjsm_node_tls;

als.run('tls-context', () => {
  tlsHost.serverListen(0, '127.0.0.1', '', '', '').then(function (server) {
    console.log('listen', als.getStore());
    const port = tlsHost.serverPort(server);
    tlsHost.serverAccept(server).then(function (socket) {
      console.log('accept', als.getStore());
      tlsHost.write(socket, 'hello');
      tlsHost.end(socket);
    });
    tlsHost.connect(port, '127.0.0.1', 'localhost', false, '').then(function (client) {
      console.log('connect', als.getStore());
      tlsHost.read(client).then(function () {
        console.log('read', als.getStore());
        tlsHost.destroy(client);
        tlsHost.serverClose(server).then(function () { process.exit(0); });
      });
    });
  });
});
