console.log(String.fromCharCode("65"));
try {
  String.fromCodePoint(0xd800);
  console.log("no throw");
} catch (e) {
  console.log(e.name);
}