// super.prop inside class method (current supported super scenario).
class Base {
  greet() {
    return "hello from base";
  }
}

class Derived extends Base {
  greet() {
    return super.greet() + " + derived";
  }
}

const d = new Derived();
console.log(d.greet());
