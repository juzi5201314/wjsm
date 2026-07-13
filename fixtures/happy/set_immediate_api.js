console.log(typeof setImmediate);
console.log(typeof clearImmediate);
const t = setImmediate(() => {
  console.log('imm');
});
console.log(typeof t);
console.log(t.constructor && t.constructor.name);
