function getHost() {
  const host = globalThis.__wjsm_node_buffer;
  if (!host) throw new Error('wjsm internal buffer host bridge is not installed');
  return host;
}

export const Buffer = globalThis.Buffer;

export function transcode(source, fromEnc, toEnc) {
  return getHost().transcode(source, fromEnc, toEnc);
}

const buffer = { Buffer, transcode };
export default buffer;
