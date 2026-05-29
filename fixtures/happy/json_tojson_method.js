// Custom toJSON method on object.
const obj = {
  value: 42,
  toJSON() {
    return { transformed: true, value: this.value * 2 };
  }
};
console.log(JSON.stringify(obj));
