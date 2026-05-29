// super() constructor call — CURRENTLY NOT SUPPORTED (normative gap).
// Per ECMAScript spec, super() is REQUIRED in derived class constructors.
// Current implementation rejects with LoweringError "super call is not supported".
class Base {
  constructor(x) {
    this.x = x;
  }
}

class Derived extends Base {
  constructor(x) {
    super(x);  // This MUST produce a compile-time diagnostic error
    this.y = 1;
  }
}

const d = new Derived(5);
console.log(d.x);
