// Console inside async function.
async function asyncConsole() {
  console.log("async-before-await");
  await Promise.resolve();
  console.info("async-after-await");
  console.debug("async-debug");
}

asyncConsole();
console.log("main-after-async-invoke");
