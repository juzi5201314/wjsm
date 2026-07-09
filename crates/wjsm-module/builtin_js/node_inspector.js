// node:inspector — Node 兼容的最小 API。
// CDP 服务由 CLI `--inspect` / `--inspect-brk` 拥有；本模块不独立起服。
// 运行时若在 globalThis.__wjsm_inspector_url 写入地址，则 url() 返回该值。

let _url = undefined;

/**
 * 打开 inspector 端口。
 * wjsm 当前由 CLI 拥有 CDP 服务：无 bridge 时为 no-op，并尽量读取已有 URL。
 * @param {number} [port]
 * @param {string} [host]
 * @param {boolean} [wait]
 */
export function open(port, host, wait) {
  // 预留：未来可通过 host bridge 动态起服。当前仅同步占位 URL。
  void port;
  void host;
  void wait;
  if (typeof globalThis.__wjsm_inspector_url === "string") {
    _url = globalThis.__wjsm_inspector_url;
  }
}

/** 关闭 inspector 服务。CLI 拥有的服务不由此模块关闭。 */
export function close() {
  // 预留：未来通过 host bridge 关闭动态起服的实例。
}

/** 返回当前 inspector URL；未启用时为 undefined。 */
export function url() {
  if (typeof globalThis.__wjsm_inspector_url === "string") {
    _url = globalThis.__wjsm_inspector_url;
  }
  return _url;
}

const inspector = { open, close, url };
export default inspector;
