// toJSON can break circular reference.
const obj = { a: 1 };
obj.self = obj;
obj.toJSON = function() {
  return { a: this.a, broken: true };
};
console.log(JSON.stringify(obj));
