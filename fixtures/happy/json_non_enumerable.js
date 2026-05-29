// Non-enumerable properties are not serialized.
const obj = {};
Object.defineProperty(obj, "hidden", { value: 42, enumerable: false, writable: true });
obj.visible = 1;
console.log(JSON.stringify(obj));
