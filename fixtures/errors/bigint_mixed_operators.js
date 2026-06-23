function check(label, fn) {
  try {
    fn();
    console.log(label, "no throw");
  } catch (e) {
    console.log(label, e.name);
  }
}
check("7n-3", function () { 7n - 3; });
check("7n*3", function () { 7n * 3; });
check("7n/3", function () { 7n / 3; });
check("3-7n", function () { 3 - 7n; });
check("3*7n", function () { 3 * 7n; });
check("3/7n", function () { 3 / 7n; });