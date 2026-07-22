function createRegistry() {
  const values = new Set();
  return {
    add(value) {
      values.add(value);
    },
    has(value) {
      return values.has(value);
    },
  };
}

const registry = createRegistry();
const value = {};
registry.add(value);
gc();
console.log(registry.has(value));
