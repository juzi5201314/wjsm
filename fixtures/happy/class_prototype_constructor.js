// 类 prototype.constructor
class Foo { constructor() {} bar() {} }
console.log(Foo.prototype.constructor === Foo); // true
console.log(typeof Foo.prototype);              // object
