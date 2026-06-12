"use strict";
function f() {
    try {
        arguments.callee;
        console.log("direct:no_throw");
    } catch (e) {
        console.log("direct:" + e.name);
    }

    try {
        Reflect.get(arguments, "callee");
        console.log("reflect:no_throw");
    } catch (e) {
        console.log("reflect:" + e.name);
    }

    const desc = Object.getOwnPropertyDescriptor(arguments, "callee");
    console.log("descriptor:" + typeof desc.get + ":" + typeof desc.set);
    console.log("flags:" + desc.enumerable + ":" + desc.configurable);

    try {
        desc.get.call(arguments);
        console.log("getter_call:no_throw");
    } catch (e) {
        console.log("getter_call:" + e.name);
    }

    try {
        desc.set.call(arguments, f);
        console.log("setter_call:no_throw");
    } catch (e) {
        console.log("setter_call:" + e.name);
    }
}
f();