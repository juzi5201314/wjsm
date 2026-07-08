// 函数 prototype 属性
function Foo() { this.x = 42; }
console.log(typeof Foo.prototype);           // object
console.log(Foo.prototype.constructor === Foo); // true

// prototype 方法赋值
Foo.prototype.bar = function() { return this.x; };
var f = new Foo();
console.log(f.bar());                        // 42

// instanceof
console.log(f instanceof Foo);               // true
console.log(f.constructor === Foo);          // true

// 原型链
console.log(Object.getPrototypeOf(f) === Foo.prototype); // true

// 原型属性继承
Foo.prototype.greet = function() { return "hello"; };
console.log(f.greet());                      // hello

// 箭头函数无 prototype
var arrow = () => {};
console.log(typeof arrow.prototype);         // undefined

// 闭包 prototype 访问
function outer() {
  var captured = 99;
  function inner() { return captured; }
  console.log(typeof inner.prototype);       // object
  console.log(inner.prototype.constructor === inner); // true
  return inner;
}
outer();
