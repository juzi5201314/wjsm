// ES class private getters/setters (issue #261)
class Foo {
  #x = 0;
  get #value() {
    return this.#x * 2;
  }
  set #value(v) {
    this.#x = v;
  }
  show() {
    return this.#value;
  }
  bump() {
    this.#value = 3;
    return this.#x;
  }
}

const f = new Foo();
console.log(f.show());
console.log(f.bump());
console.log(f.show());