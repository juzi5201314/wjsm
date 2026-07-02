const obj = {
  prefix: "obj",
  *gen(step) {
    yield this.prefix + ":" + step;
  },
};

class Counter {
  constructor(start) {
    this.start = start;
  }

  *gen() {
    yield this.start;
    yield this.start + 1;
  }
}

const objIter = obj.gen(1);
const detached = obj.gen.call({ prefix: "call" }, 2);
const classIter = new Counter(4).gen();

const objR1 = objIter.next();
const objR2 = objIter.next();
const callR1 = detached.next();
const callR2 = detached.next();
const classR1 = classIter.next();
const classR2 = classIter.next();

console.log("obj", objR1.value, objR2.done);
console.log("call", callR1.value, callR2.done);
console.log("class1", classR1.value, classR1.done);
console.log("class2", classR2.value, classR2.done);
