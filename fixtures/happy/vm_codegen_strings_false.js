// codeGeneration.strings:false 不拦截 runIn* / Script / compileFunction，
// 但拦截 context 内 eval / Function 构造器。
const vm = require("vm");

const ctx = vm.createContext({ x: 40 }, { codeGeneration: { strings: false } });

console.log("runInContext", vm.runInContext("x + 2", ctx));

const script = new vm.Script("x + 3");
console.log("script", script.runInContext(ctx));

const fn = vm.compileFunction("return x + 1", [], { parsingContext: ctx });
console.log("compileFunction", fn());

// free vars resolve to realm builtins
console.log("typeof_Promise", vm.runInContext("typeof Promise", ctx));
console.log("typeof_eval", vm.runInContext("typeof eval", ctx));

let evalOk = false;
try {
  vm.runInContext("eval('1+1')", ctx);
  evalOk = true;
} catch (e) {
  console.log("eval_blocked", e && e.name ? e.name : "Error");
}
if (evalOk) console.log("eval_blocked", "NONE");

let fnOk = false;
try {
  vm.runInContext("Function('return 1')()", ctx);
  fnOk = true;
} catch (e) {
  console.log("function_blocked", e && e.name ? e.name : "Error");
}
if (fnOk) console.log("function_blocked", "NONE");

// contextCodeGeneration 别名
const ctx2 = vm.createContext(
  {},
  { contextCodeGeneration: { strings: false } }
);
try {
  vm.runInContext("eval('2')", ctx2);
  console.log("alias_eval", "NONE");
} catch (e) {
  console.log("alias_eval", e && e.name ? e.name : "Error");
}
