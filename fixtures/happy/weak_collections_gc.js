var registry = new FinalizationRegistry(function (heldValue) {
    console.log('collected:', heldValue);
});
var weakMap = new WeakMap();
var weakSet = new WeakSet();

(function () {
    var mapKey = { kind: 'map' };
    var setKey = { kind: 'set' };
    weakMap.set(mapKey, { value: 1 });
    weakSet.add(setKey);
    registry.register(mapKey, 'weakmap-key');
    registry.register(setKey, 'weakset-key');
    console.log(weakMap.has(mapKey));
    console.log(weakSet.has(setKey));
})();

gc();
console.log('after gc');
