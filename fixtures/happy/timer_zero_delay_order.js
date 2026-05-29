// Timer FIFO order: multiple setTimeout(0) should execute in registration order.
// KNOWN-BROKEN: Timer callbacks do not currently fire — output only shows main-thread execution.
console.log("start");

setTimeout(() => console.log("first"), 0);
setTimeout(() => console.log("second"), 0);
setTimeout(() => console.log("third"), 0);

console.log("end");
