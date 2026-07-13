const dgram = require('node:dgram');
const { AsyncLocalStorage } = require('node:async_hooks');
const als = new AsyncLocalStorage();

let server;
let client;
als.run('udp-context', () => {
  server = dgram.createSocket('udp4');
  client = dgram.createSocket('udp4');
  server.on('message', (msg, rinfo) => {
    console.log('message', als.getStore());
    client.send(Buffer.from('pong'), rinfo.port, '127.0.0.1');
  });
  client.on('message', () => {
    console.log('reply', als.getStore());
    client.close();
    server.close();
    process.exit(0);
  });
  server.bind(0, '127.0.0.1', () => {
    console.log('bind', als.getStore());
    const port = server.address().port;
    client.bind(0, '127.0.0.1', () => {
      client.send(Buffer.from('ping'), port, '127.0.0.1');
    });
  });
});
