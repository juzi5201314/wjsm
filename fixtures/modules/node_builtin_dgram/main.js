const dgram = require('node:dgram');
console.log(typeof dgram.createSocket, typeof dgram.Socket);

const server = dgram.createSocket('udp4');
const client = dgram.createSocket('udp4');

server.on('message', (msg, rinfo) => {
  console.log('server received:', msg != null && rinfo.port > 0);
  client.send(Buffer.from('pong'), rinfo.port, '127.0.0.1');
});

client.on('message', (msg, rinfo) => {
  console.log('client received:', msg != null && rinfo.port > 0);
  client.close();
  server.close();
  console.log('closed');
  process.exit(0);
});

server.bind(0, '127.0.0.1', () => {
  const port = server.address().port;
  console.log('listening', port > 0);
  client.bind(0, '127.0.0.1', () => {
    client.send(Buffer.from('ping'), port, '127.0.0.1');
  });
});
