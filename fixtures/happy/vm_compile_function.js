const vm = require("vm");

const add = vm.compileFunction("return a + b", ["a", "b"]);
console.log("basic", add(1, 2));
console.log("length", add.length);
console.log("reuse", add(3, 4), add(10, 20));

const multi = vm.compileFunction("var t = 1; return t + b", ["b"]);
console.log("multi", multi(2));

globalThis.foo = 99;
const free = vm.compileFunction("return foo");
console.log("free_main", free());

const sandbox = { x: 10 };
vm.createContext(sandbox);
const withCtx = vm.compileFunction("return x + y", ["y"], {
  parsingContext: sandbox,
});
console.log("ctx", withCtx(5));
sandbox.x = 100;
console.log("ctx_live", withCtx(1));

const ext = { z: 3 };
const withExt = vm.compileFunction("return z + a", ["a"], {
  contextExtensions: [ext],
});
console.log("ext", withExt(4));

const e1 = { a: 1 };
const e2 = { b: 2 };
const multiExt = vm.compileFunction("return a + b", [], {
  contextExtensions: [e1, e2],
});
console.log("exts", multiExt());

const typeofMissing = vm.compileFunction("return typeof missing", [], {
  parsingContext: sandbox,
});
console.log("typeof_missing", typeofMissing());

try {
  const boom = vm.compileFunction("return missing");
  boom();
  console.log("miss_no_throw");
} catch (e) {
  console.log("miss_err", e.name, e.message);
}

try {
  vm.compileFunction("return 1", [1]);
  console.log("badparam_no_throw");
} catch (e) {
  console.log("badparam", e.name, e.message);
}

try {
  vm.compileFunction("return 1", [], { parsingContext: {} });
  console.log("notctx_no_throw");
} catch (e) {
  console.log("notctx", e.name);
}

try {
  vm.compileFunction("return {{{");
  console.log("syntax_no_throw");
} catch (e) {
  console.log("syntax", e.name);
}
