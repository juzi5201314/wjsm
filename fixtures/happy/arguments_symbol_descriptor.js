function f() {
    const desc = Object.getOwnPropertyDescriptor(arguments, Symbol.iterator);
    console.log(typeof desc.value);
    console.log(desc.value === Array.prototype.values);
    console.log(desc.writable);
    console.log(desc.enumerable);
    console.log(desc.configurable);
}
f(1, 2);
