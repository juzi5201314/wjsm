try {
  JSON.parse("1.");
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
}
