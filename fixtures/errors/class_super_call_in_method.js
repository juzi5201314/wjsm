class Base {}
class Derived extends Base {
  method() {
    super();
  }
}
new Derived().method();
