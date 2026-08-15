function add1(x) {
  return x + 1;
}

function twice(x) {
  return add1(add1(x));
}
console.log(twice(40.5));
console.log(twice("a"));
