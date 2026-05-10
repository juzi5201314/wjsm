async function demo() {
  var a = await Promise.resolve(10);
  if (a > 5) {
    console.log("big");
  } else {
    console.log("small");
  }
  var b = await Promise.resolve(3);
  if (b > 5) {
    console.log("big");
  } else {
    console.log("small");
  }
}
demo();
