async function boom() {
  throw "error";
}
boom().catch(e => console.log(e));
