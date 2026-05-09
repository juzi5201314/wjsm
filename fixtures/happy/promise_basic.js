let result = 0;
let p = new Promise(function(resolve) {
  resolve(42);
});
p.then(function(v) {
  result = v;
  console.log(result);
});
