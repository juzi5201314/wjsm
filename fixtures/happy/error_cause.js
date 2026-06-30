// ES2022 Error.cause support (issue #147)
const inner = new Error("inner");
const outer = new Error("outer", { cause: inner });
console.log(outer.cause === inner);
console.log(outer.cause.message);

// TypeError with cause
const te = new TypeError("type", { cause: "string cause" });
console.log(te.cause);

// Error() without new
const e = Error("msg", { cause: 42 });
console.log(e.cause);

// No cause
const noCause = new Error("nocause");
console.log(noCause.cause);

// Undefined cause
const undefCause = new Error("uc", { cause: undefined });
console.log(undefCause.cause);

// Nested cause chain
const e1 = new Error("e1");
const e2 = new Error("e2", { cause: e1 });
const e3 = new Error("e3", { cause: e2 });
console.log(e3.cause === e2);
console.log(e3.cause.cause === e1);
