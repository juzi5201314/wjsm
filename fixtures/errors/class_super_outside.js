// super used outside any class context — must be early error.
function notInClass() {
  return super.foo;
}
console.log(notInClass());
