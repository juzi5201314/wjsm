// Test Object.getOwnPropertyDescriptor with data properties

const obj = { value: 42 };
const desc = Object.getOwnPropertyDescriptor(obj, 'value');
console.log(desc.value);
console.log(desc.writable);
console.log(desc.enumerable);
console.log(desc.configurable);
