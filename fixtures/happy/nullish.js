let hit = 0;
console.log(0 ?? (hit = 1));
console.log(false ?? (hit = 2));
console.log("x" ?? (hit = 3));
console.log(null ?? 4);
console.log(void 0 ?? 5);
console.log(hit);
