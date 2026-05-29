// String escaping edge cases.
const s1 = "quote\"back\\slash";
const s2 = "tab\there\nnewline";
const s3 = "emoji😀unicode";
console.log(JSON.stringify(s1));
console.log(JSON.stringify(s2));
console.log(JSON.stringify(s3));
