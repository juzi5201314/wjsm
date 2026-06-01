// Invalid JSON must produce a SyntaxError observable by catch.
try {
  const bad = JSON.parse("{not valid json");
  console.log("no-throw:", bad);
} catch (e) {
  console.log("threw:", e.message);
}
