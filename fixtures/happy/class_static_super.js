class Base {
  static greet() {
    return "base";
  }
}

class Derived extends Base {
  static greet() {
    return super.greet() + " derived";
  }
}

console.log(Derived.greet());
console.log(Object.getPrototypeOf(Derived) === Base);
