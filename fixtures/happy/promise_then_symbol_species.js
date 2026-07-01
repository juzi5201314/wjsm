// #163: Promise subclass with static [Symbol.species] => Promise; .then() uses species.
class MyPromise extends Promise {
  static get [Symbol.species]() {
    return Promise;
  }
}

const p = Promise.resolve(1);
const chained = p.then((v) => v + 1);
console.log(chained instanceof Promise);
console.log(chained instanceof MyPromise);

const sub = new MyPromise((resolve) => resolve(2));
const subChained = sub.then((v) => v);
console.log(subChained instanceof Promise);
console.log(subChained instanceof MyPromise);

// 基本 fulfill/reject 仍可用
Promise.resolve(10).then((v) => console.log(v));
Promise.reject("err").catch((e) => console.log(e));