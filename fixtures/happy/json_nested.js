// Deeply nested structure.
const nested = {
  a: {
    b: {
      c: [1, { d: { e: "deep" } }]
    }
  }
};
console.log(JSON.stringify(nested));
