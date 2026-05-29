// Invalid JSON — per spec must throw SyntaxError.
// Current stub may not throw or may return garbage. Fixture documents actual behavior.
try {
  const bad = JSON.parse("{not valid json");
  console.log("no-throw:", bad);
} catch (e) {
  console.log("threw:", e.message);
}
