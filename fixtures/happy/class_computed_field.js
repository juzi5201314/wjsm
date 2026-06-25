// Computed instance and static class field names (issue #260)
const key = "myField";
class Foo {
  [key] = 42;
}
console.log(new Foo().myField);

const staticKey = "onClass";
class Bar {
  static [staticKey] = 7;
}
console.log(Bar.onClass);