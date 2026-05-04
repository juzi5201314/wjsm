function counter() {
  var count = 0;
  return function() {
    count = count + 1;
    return count;
  };
}
var inc = counter();
console.log(inc());
console.log(inc());
console.log(inc());
