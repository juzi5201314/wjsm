function f(a, b) {
  return this.x + a + (b || 0);
}

const o = { x: 10 };
const bound = f.bind(o, 1);
console.log(bound(2));
console.log(f.call(o, 3, 4));
console.log(f.apply(o, [5, 6]));

const host = {
  bind() {
    return "host-bind";
  },
  call() {
    return "host-call";
  },
  apply() {
    return "host-apply";
  },
};
console.log(host.bind());
console.log(host.call());
console.log(host.apply());

const extracted = f.bind;
console.log(typeof extracted);
console.log(extracted.call(f, { x: 100 }, 7)());
