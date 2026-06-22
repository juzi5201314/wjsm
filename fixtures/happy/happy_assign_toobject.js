// Object.assign applies ToObject to string sources (#188)
const out = Object.assign({}, "ab");
console.log(out[0], out[1], out.length);