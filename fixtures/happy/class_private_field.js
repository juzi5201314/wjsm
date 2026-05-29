// Private field.
class Counter {
  #count = 0;
  
  inc() {
    this.#count++;
    return this.#count;
  }
  
  getCount() {
    return this.#count;
  }
}

const c = new Counter();
console.log(c.inc());
console.log(c.getCount());
