export function load() {
  let loaded;
  loaded = import('./dep.mjs');
  return loaded.then(ns => ns.value);
}
