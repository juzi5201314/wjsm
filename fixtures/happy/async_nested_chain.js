async function c() {
  return 3;
}
async function b() {
  const v = await c();
  return v + 1;
}
async function a() {
  const v = await b();
  return v + 1;
}
a().then(v => console.log(v));
