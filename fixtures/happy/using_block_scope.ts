function makeDisposable(name: string) {
  return {
    name: name,
    [Symbol.dispose]: function() {
      console.log("dispose");
    },
  };
}

{
  using a = makeDisposable("a");
  console.log("using");
}
console.log("after");
