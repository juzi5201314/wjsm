function Foo() {
  return { custom: true };
}
console.log(JSON.stringify(new Foo()));

class Bar {
  constructor() {
    return { override: true };
  }
}
console.log(JSON.stringify(new Bar()));

function Baz() {
  return 42;
}
console.log(JSON.stringify(new Baz()));

function Qux() {
  return undefined;
}
console.log(JSON.stringify(new Qux()));