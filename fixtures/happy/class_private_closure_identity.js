let privateValue = 1;

class PrivateClosure {
  #captured(next) {
    privateValue = next;
    return privateValue;
  }

  #empty() {
    return 1;
  }

  get #shared() {
    return privateValue;
  }

  set #shared(next) {
    privateValue = next;
  }

  static get #staticShared() {
    return privateValue;
  }

  static set #staticShared(next) {
    privateValue = next;
  }

  static #read() {
    return privateValue;
  }

  capturedMethod() {
    return this.#captured;
  }

  emptyMethod() {
    return this.#empty;
  }

  read() {
    return this.#shared;
  }

  write(next) {
    this.#shared = next;
  }

  static read() {
    return this.#read();
  }

  static readStatic() {
    return this.#staticShared;
  }

  static writeStatic(next) {
    this.#staticShared = next;
  }
}

const first = new PrivateClosure();
const second = new PrivateClosure();
console.log(first.capturedMethod() === second.capturedMethod());
console.log(first.emptyMethod() === second.emptyMethod());
first.write(2);
console.log(second.read() === 2);
const captured = first.capturedMethod();
console.log(captured(3) === 3 && second.read() === 3);
PrivateClosure.writeStatic(4);
console.log(PrivateClosure.readStatic() === 4 && PrivateClosure.read() === 4);
