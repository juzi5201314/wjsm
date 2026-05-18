"use strict";
function f(a, b) {
    console.log(typeof arguments);
    console.log(Object.prototype.toString.call(arguments));
}
f(1, 2);
