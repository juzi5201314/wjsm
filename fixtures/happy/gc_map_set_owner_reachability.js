// Issue #334：不可达 Map/Set 的侧表条目不能把 key/value 永久保活。
var registry = new FinalizationRegistry(function (held) {
  console.log("cleaned", held);
});

(function () {
  let map = new Map();
  let value = { marker: "map" };
  registry.register(value, "map-value");
  map.set("k", value);
})();

(function () {
  let set = new Set();
  let value = { marker: "set" };
  registry.register(value, "set-value");
  set.add(value);
})();

gc();
console.log("after gc");
