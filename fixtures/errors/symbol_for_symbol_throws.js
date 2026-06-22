try {
  Symbol.for(Symbol("x"));
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
}