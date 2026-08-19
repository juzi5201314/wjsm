const BASE = "the quick brown fox jumps over the lazy dog";

let text = "";
for (let index = 0; index < 100; index++) {
  text += BASE + index;
}

console.log(text.length === 4490);
console.log(text.slice(0, 44) === BASE + "0");
console.log(text.slice(-45) === BASE + "99");
