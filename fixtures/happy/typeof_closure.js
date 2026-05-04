function makeClosure() {
  var x = 1;
  return function() {
    return x;
  };
}

var closure = makeClosure();
console.log(typeof closure);
console.log(JSON.stringify(closure));
