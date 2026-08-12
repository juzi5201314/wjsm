const a = [1, 2, 3, 4, 5];
a.length = 3;
console.log(a);
a.length = 5;
console.log(a);
console.log(a.length);

const holes = new Array(3);
console.log(holes.length, 0 in holes, holes[0]);

const single = new Array("3");
console.log(single.length, single[0]);

for (const length of [-1, 1.5, NaN, Infinity]) {
  try {
    new Array(length);
  } catch (error) {
    console.log(error.name);
  }
}