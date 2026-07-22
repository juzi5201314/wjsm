const net = require('node:net');

let failures = 0;
let server;

function finish() {
  if (failures !== 2) return;
  console.log(true);
  server.close();
}

function recordFetchFailure() {
  failures = failures + 1;
  finish();
}

function recordNetFailure() {
  failures = failures + 1;
  fetch('http://127.0.0.1:1/unreachable').catch(recordFetchFailure);
}

function connected(socket) {
  socket.destroy();
  gc();
  setImmediate(() => {
    fetch('data:text/plain,hello').then((response) => response.text()).then(runFailure);
  });
}

function runFailure() {
  const socket = net.connect(1, '127.0.0.1');
  socket.on('error', recordNetFailure);
}

server = net.createServer((socket) => socket.end());
server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => connected(socket));
});
