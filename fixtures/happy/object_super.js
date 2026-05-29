const proto = {
  value() {
    return 2;
  },
  get x() {
    return this.y + 1;
  }
};

const obj = {
  __proto__: proto,
  y: 4,
  value() {
    return super.value() + 3;
  },
  get x() {
    return super.x + 1;
  }
};

console.log(obj.value());
console.log(obj.x);
