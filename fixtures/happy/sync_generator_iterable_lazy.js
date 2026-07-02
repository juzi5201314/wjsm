let counter = 0;
function* gen() {
  counter = counter + 1;
  yield counter;
  counter = counter + 1;
  yield counter;
}

const g = gen();
console.log("before", counter);
let r = g.next();
console.log("first", r.value, counter);
r = g.next();
console.log("second", r.value, counter);

let total = 0;
for (const value of gen()) {
  total = total + value;
}
console.log("for", total);
console.log("spread", [...gen()].join("-"));
const [a, b] = gen();
console.log("destructure", a, b);
