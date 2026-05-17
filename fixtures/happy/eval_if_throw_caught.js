let result = "start";
try {
  if (eval("throw 'boom'")) {
    result = "then";
  }
  result = "after";
} catch (e) {
  result = e;
}
console.log(result);
