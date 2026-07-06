function getHost() {
  const host = globalThis.__wjsm_node_crypto;
  if (!host) throw new Error('wjsm internal crypto host bridge is not installed');
  return host;
}
const host = getHost();


export function randomBytes(size) { return host.randomBytes(size); }
export function randomUUID() { return host.randomUUID(); }
export function randomInt(min, max) { return host.randomInt(min, max); }
export function createHash(algorithm) { return host.createHash(algorithm); }
export function createHmac(algorithm, key) { return host.createHmac(algorithm, key); }
export function timingSafeEqual(a, b) { return host.timingSafeEqual(a, b); }
export function getHashes() { return ['md5', 'sha1', 'sha256', 'sha512']; }

const crypto = {
  randomBytes(size) { return randomBytes(size); },
  randomUUID() { return randomUUID(); },
  randomInt(min, max) { return randomInt(min, max); },
  createHash(algorithm) { return createHash(algorithm); },
  createHmac(algorithm, key) { return createHmac(algorithm, key); },
  timingSafeEqual(a, b) { return timingSafeEqual(a, b); },
  getHashes() { return getHashes(); },
};
export default crypto;
