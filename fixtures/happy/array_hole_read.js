const a = [1, , 3];
console.log(a[1]);
console.log(a[1] === undefined);
console.log(typeof a[1]);
console.log(1 in a);
console.log(0 in a);
console.log(a[1] ?? 'hole');
for (let i = 0; i < a.length; i++) {
  console.log(a[i]);
}
const [x, y, z] = a;
console.log(x);
console.log(y);
console.log(z);
for (const v of a) {
  console.log(v);
}
const it = a[Symbol.iterator]();
console.log(it.next().value);
console.log(it.next().value);
console.log(it.next().value);
console.log([...a]);
function collect() {
  console.log(arguments.length, arguments[1] === undefined);
}
collect.apply(null, [1, , 3]);
collect(...[1, , 3]);
