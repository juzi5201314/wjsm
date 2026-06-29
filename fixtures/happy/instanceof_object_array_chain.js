// Object/Array 原生构造器的 .prototype 必须能被 instanceof 反射路径读取。
console.log(({}) instanceof Object);
console.log([] instanceof Array);
console.log([] instanceof Object);

// bootstrap 必须建立 Array.prototype -> Object.prototype 原型链。
console.log(Object.getPrototypeOf(Array.prototype) === Object.prototype);

// TAG_REGEXP 单独装箱，但语义上继承 RegExp.prototype -> Object.prototype。
console.log(Object.getPrototypeOf(/x/) === RegExp.prototype);
console.log(/x/ instanceof RegExp);
console.log(/x/ instanceof Object);
