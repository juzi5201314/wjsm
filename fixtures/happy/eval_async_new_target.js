let p;
function C() {
  p = (async () => {
    const nt = eval('new.target');
    console.log(typeof nt);
    console.log(nt === C);
  })();
}
new C();
await p;
