const net = require('node:net');
const { AsyncLocalStorage } = require('node:async_hooks');
const als = new AsyncLocalStorage();
let acceptStore;
let connectStore;

let server;
als.run('net-context', () => {
  server = net.createServer((socket) => {
    acceptStore = als.getStore();
    socket.end('ping');
  });
  server.listen(0, '127.0.0.1', () => {
    console.log('listen', als.getStore());
    var client = net.createConnection(server.address().port, '127.0.0.1', () => {
      connectStore = als.getStore();
    });
    client.on('end', () => {
      console.log('accept', acceptStore);
      console.log('connect', connectStore);
      client.destroy();
      server.close();
      process.exit(0);
    });
  });
});
