// for-of 按码点迭代
const forOf = [];
for (const ch of "héllo🌍") {
  forOf.push(ch);
}
console.log("for-of count:", forOf.length);
console.log("for-of:", JSON.stringify(forOf));

console.log("spread hello:", JSON.stringify([..."héllo"]));
console.log("spread emoji:", JSON.stringify([..."🌍"]));
console.log("spread ascii:", JSON.stringify([..."abc"]));

const empty = "";
let emptyDone = false;
for (const _ of empty) {
  emptyDone = true;
}
console.log("empty iter ran:", emptyDone);