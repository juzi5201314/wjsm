const net = require('node:net');
console.log(typeof net.createServer, typeof net.Socket, net.isIP('127.0.0.1'));

var server = net.createServer((socket) => {
  socket.end('ping');
});

server.listen(0, '127.0.0.1', () => {
  console.log('listening', server.address().port > 0);
  const port = server.address().port;
  var client = net.createConnection(port, '127.0.0.1', () => {
    console.log('connected');
    client.destroy();
    server.close();
    console.log('closed');
    process.exit(0);
  });
});
