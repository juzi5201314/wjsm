class Counter {
  constructor() {
    this.n = 0;
  }
  inc() {
    this.n = this.n + 1;
    return this.n;
  }
}
let c = new Counter();
console.log(c.inc());
console.log(c.inc());
