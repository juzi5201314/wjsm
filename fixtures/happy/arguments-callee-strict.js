// KNOWN-BROKEN: strict mode arguments.callee should throw TypeError
// Current behavior logs no_throw before surfacing an exception to catch.
"use strict";
function f() {
    try {
        arguments.callee;
        console.log("no_throw");
    } catch (e) {
        console.log("throw");
    }
}
f();