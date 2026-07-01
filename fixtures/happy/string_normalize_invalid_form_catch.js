// #179: invalid form is catchable RangeError (like Number.toExponential)
try {
  "x".normalize("BOGUS");
  console.log("no_throw");
} catch (e) {
  console.log(e.name);
}