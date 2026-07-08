const dgram = require('node:dgram');
console.log(typeof dgram.createSocket, typeof dgram.Socket);

const host = globalThis.__wjsm_node_dgram;
const bind = host.bind;

bind(0, '127.0.0.1').then(function (server) {
  const port = host.port(server);
  console.log('listening', port > 0);
  bind(0, '127.0.0.1').then(function (client) {
    const clientPort = host.port(client);
    host.recv(server).then(function (packet) {
      console.log('server received:', packet !== null && packet.port > 0);
      host.recv(client).then(function (reply) {
        console.log('client received:', reply !== null && reply.port > 0);
        host.close(client);
        host.close(server);
        console.log('closed');
        process.exit(0);
      });
      host.send(server, 'pong', clientPort, '127.0.0.1');
    });
    host.send(client, 'ping', port, '127.0.0.1');
  });
});
