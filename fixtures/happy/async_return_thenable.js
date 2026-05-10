async function foo() {
  return { then: function(resolve) { resolve(99); } };
}
foo().then(v => console.log(v));
