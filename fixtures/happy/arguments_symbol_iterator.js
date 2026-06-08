function f() {
    console.log(typeof arguments[Symbol.iterator]);
    console.log(arguments[Symbol.iterator] === Array.prototype.values);
}
f(1, 2);
