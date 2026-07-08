import { Transform } from 'stream';

function getHost() {
  const host = globalThis.__wjsm_node_zlib;
  if (!host) throw new Error('wjsm internal zlib host bridge is not installed');
  return host;
}

function normalizeOptions(options) { return options && typeof options === 'object' ? options : {}; }

export function gzipSync(buffer, options) { return getHost().gzipSync(buffer, normalizeOptions(options)); }
export function gunzipSync(buffer, options) { return getHost().gunzipSync(buffer, normalizeOptions(options)); }
export function deflateSync(buffer, options) { return getHost().deflateSync(buffer, normalizeOptions(options)); }
export function inflateSync(buffer, options) { return getHost().inflateSync(buffer, normalizeOptions(options)); }
export function deflateRawSync(buffer, options) { return getHost().deflateRawSync(buffer, normalizeOptions(options)); }
export function inflateRawSync(buffer, options) { return getHost().inflateRawSync(buffer, normalizeOptions(options)); }
export function brotliCompressSync(buffer, options) { return getHost().brotliCompressSync(buffer, normalizeOptions(options)); }
export function brotliDecompressSync(buffer, options) { return getHost().brotliDecompressSync(buffer, normalizeOptions(options)); }

export function createGzip(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = gzipSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createGunzip(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = gunzipSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createDeflate(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = deflateSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createInflate(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = inflateSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createDeflateRaw(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = deflateRawSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createInflateRaw(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = inflateRawSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createBrotliCompress(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = brotliCompressSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }
export function createBrotliDecompress(options) { return new Transform({ transform: function (chunk, encoding, callback) { callback(null, chunk); }, flush: function (callback) { try { const input = Buffer.concat(this._zlibChunks || []); const out = brotliDecompressSync(input, options); if (out && out.length !== 0) this.push(out); callback(); } catch (e) { callback(e); } } }); }

export const constants = { Z_NO_FLUSH: 0, Z_FINISH: 4, Z_OK: 0, Z_STREAM_END: 1 };

const zlib = {
  gzipSync: gzipSync,
  gunzipSync: gunzipSync,
  deflateSync: deflateSync,
  inflateSync: inflateSync,
  deflateRawSync: deflateRawSync,
  inflateRawSync: inflateRawSync,
  brotliCompressSync: brotliCompressSync,
  brotliDecompressSync: brotliDecompressSync,
  createGzip: createGzip,
  createGunzip: createGunzip,
  createDeflate: createDeflate,
  createInflate: createInflate,
  createDeflateRaw: createDeflateRaw,
  createInflateRaw: createInflateRaw,
  createBrotliCompress: createBrotliCompress,
  createBrotliDecompress: createBrotliDecompress,
  constants: constants,
};
export default zlib;
