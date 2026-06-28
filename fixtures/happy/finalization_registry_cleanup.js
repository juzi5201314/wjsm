var registry = new FinalizationRegistry(function (heldValue) {
    console.log('cleaned:', heldValue);
});

(function () {
    var target = { name: 'target' };
    registry.register(target, 'held-value');
})();

gc();
console.log('after gc');
