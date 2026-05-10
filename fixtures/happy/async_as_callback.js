async function double(x) {
  return x * 2;
}
Promise.resolve(5).then(double).then(v => console.log(v));
Promise.resolve(10).then(double).then(v => console.log(v));
