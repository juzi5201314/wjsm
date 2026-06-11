// Private methods & fields (ECMAScript §13.3): internal access works; the runtime
// brand check throws a catchable TypeError when a private member is accessed on an
// object that is not an instance of the declaring class.
//
// (External `s.#hidden` outside the class is an *early SyntaxError* — see
//  errors/class_private_method_outside.js.)
class Secret {
  #hidden() {
    return "secret";
  }
  #data = 42;

  reveal() {
    return this.#hidden(); // internal private method call
  }
  getData() {
    return this.#data; // internal private field read
  }
  // `#data` is lexically in scope here, but `o` may not carry the brand → runtime check.
  static probe(o) {
    return o.#data;
  }
}

const s = new Secret();
console.log(s.reveal());      // "secret"
console.log(s.getData());     // 42
console.log(Secret.probe(s)); // 42 — s carries the #data brand

try {
  Secret.probe({}); // a plain object has no #data brand → TypeError
  console.log("FAIL: brand check did not throw");
} catch (e) {
  console.log("brand-check-error");
}
