try {
  JSON.parse(Symbol.iterator);
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
  console.log("caught-type:", typeof e);
}
