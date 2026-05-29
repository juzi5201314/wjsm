// Private method — internal access works; external access is not spec-valid syntax.
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

// NOTE: External private field access should throw TypeError per spec, but the
// runtime currently allows it (returns the function reference). This test
// documents the actual behavior including the catch path.
try {
  console.log(s.#hidden);
} catch (e) {
  console.log("direct-private-access-error");
}
