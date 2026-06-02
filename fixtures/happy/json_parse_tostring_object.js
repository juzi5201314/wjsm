const input = {
  toString() {
    return '{"x":1}';
  },
};
const result = JSON.parse(input);
console.log("x:", result.x);
