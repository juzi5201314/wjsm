const proto = {
  get value() {
    return this.x + 1;
  }
};
const receiver = { x: 41 };
console.log(Reflect.get(proto, 'value', receiver));

const target = Object.create(proto);
target.x = 10;
console.log(Reflect.get(target, 'value'));
