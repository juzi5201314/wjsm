// Referencing a private name outside its declaring class is an early SyntaxError
// (ECMAScript §13.3.1.1 Static Semantics: AllPrivateIdentifiersValid). The whole
// script is rejected before execution, so nothing is printed to stdout.
class Secret {
  #hidden() {
    return "secret";
  }
}

const s = new Secret();
console.log(s.#hidden); // #hidden is not in any enclosing class scope here
