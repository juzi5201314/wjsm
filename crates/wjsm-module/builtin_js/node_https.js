import httpDefault, { request as httpRequest, get as httpGet, ClientRequest, IncomingMessage, Server, createServer as httpCreateServer, METHODS, STATUS_CODES, Agent } from 'http';

export { ClientRequest, IncomingMessage, Server, METHODS, STATUS_CODES, Agent };

export function request(input, options, callback) {
  return httpRequest(input, options, callback);
}

export function get(input, options, callback) {
  return httpGet(input, options, callback);
}

export function createServer() {
  return httpCreateServer();
}

export const globalAgent = new Agent({ protocol: 'https:' });
const https = { request, get, ClientRequest, IncomingMessage, Server, createServer, METHODS, STATUS_CODES, Agent, globalAgent };
export default https;
