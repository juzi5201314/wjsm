import fn from "./fn_dep.js";
import Klass from "./class_dep.js";
import arrow from "./expr_dep.js";
import named from "./named_dep.js";

console.log(fn.name, fn.length);
console.log(Klass.name, Klass.length);
console.log(arrow.name, arrow.length);
console.log(named.name, named.length);
