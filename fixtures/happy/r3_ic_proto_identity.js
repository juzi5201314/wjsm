// 同一访问点遇到相同 shape、不同直接原型时，IC 不能复用第一个 holder。
const firstDataProto = { value: "first" };
const secondDataProto = { value: "second" };
const firstData = Object.create(firstDataProto);
const secondData = Object.create(secondDataProto);

function readData(object) {
  return object.value;
}

console.log(
  "data:",
  readData(firstData),
  readData(secondData),
  readData(firstData),
  readData(secondData),
);

const firstAccessorProto = {
  get value() {
    return "first-accessor";
  },
};
const secondAccessorProto = {
  get value() {
    return "second-accessor";
  },
};
const firstAccessor = Object.create(firstAccessorProto);
const secondAccessor = Object.create(secondAccessorProto);

function readAccessor(object) {
  return object.value;
}

console.log(
  "accessor:",
  readAccessor(firstAccessor),
  readAccessor(secondAccessor),
  readAccessor(firstAccessor),
  readAccessor(secondAccessor),
);
