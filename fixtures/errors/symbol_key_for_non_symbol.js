try {
  Symbol.keyFor(42);
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
}