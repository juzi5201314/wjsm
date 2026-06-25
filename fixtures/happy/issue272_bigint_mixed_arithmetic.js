// Issue #272: Mixed BigInt/Number arithmetic must throw TypeError
function check(label, fn) {
  try {
    fn();
    console.log(label, "no throw");
  } catch (e) {
    console.log(label, e.name);
  }
}
check("1n+2", function () { 1n + 2; });
check("1n-2", function () { 1n - 2; });
check("10n*3", function () { 10n * 3; });
check("5n/2", function () { 5n / 2; });
check("5n%2", function () { 5n % 2; });
check("2n**3", function () { 2n ** 3; });
check("1n+2n", function () { return 1n + 2n; });
check("1+2", function () { return 1 + 2; });