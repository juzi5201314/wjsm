function run() {
  var x = 0;
  var hits = 0;

  function bump() {
    hits = hits + 1;
    return 1;
  }

  function update() {
    x &&= bump();
    x ||= bump();
    x ??= bump();
    return hits;
  }

  return update();
}

console.log(run());
