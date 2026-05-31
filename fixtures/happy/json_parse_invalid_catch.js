try {
  JSON.parse("{not valid json");
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
  console.log("caught-type:", typeof e);
}
