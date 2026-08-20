const net = require('node:net');
console.log(typeof net.createServer, typeof net.Socket, net.isIP('127.0.0.1'));
