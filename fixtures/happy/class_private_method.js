// Private method.
class Secret {
  #hidden() {
    return "secret";
  }
  
  reveal() {
    return this.#hidden();
  }
}

const s = new Secret();
console.log(s.reveal());
// Direct access should fail at runtime (or compile error)
try {
  console.log(s.#hidden);
} catch (e) {
  console.log("direct-private-access-error");
}
