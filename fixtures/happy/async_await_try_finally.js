async function run() {
  try {
    await Promise.reject("boom");
    console.log("unreachable");
  } catch (e) {
    console.log(e);
    return 7;
  } finally {
    console.log("finally");
  }
}
run().then(v => console.log(v));
