const { MessageChannel } = require('worker_threads');
const { port1, port2 } = new MessageChannel();
port1.on('message', (m) => {
  console.log('p1', m);
  port1.close();
  port2.close();
  process.exit(0);
});
port2.postMessage('hi');
