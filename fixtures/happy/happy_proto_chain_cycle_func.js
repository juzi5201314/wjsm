// Prototype cycle detection through function objects (#187)
function f() {}
const g = function () {};
f.__proto__ = g;
g.__proto__ = f;
const ok = Object.setPrototypeOf({}, f);
console.log(ok === false);