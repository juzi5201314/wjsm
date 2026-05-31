try {
  JSON.parse("-01");
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
}
