let a = [1, 2];
a = {};
try {
  let b = a.map(x => x * 2);
  console.log("unreached", b);
} catch (e) {
  console.log("guard-ok");
}
