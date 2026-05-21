let fr = new FinalizationRegistry((heldValue) => {
    console.log('cleaned:', heldValue);
});

console.log('registered');

// Test unregister
let obj = {};
let token = {};
fr.register(obj, 'value1', token);
let unregistered = fr.unregister(token);
console.log('unregistered:', unregistered);
