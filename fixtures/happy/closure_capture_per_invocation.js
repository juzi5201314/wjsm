// 同一工厂函数多次调用必须得到独立的捕获环境，
// 不能把参数写进共享的父 env 导致互相覆盖。
function makeArm(handle) {
  return function () {
    return Promise.resolve(handle).then(function (h) {
      return h;
    });
  };
}

const a0 = makeArm(0);
const a1 = makeArm(1);
const a2 = makeArm(42);

Promise.all([a0(), a1(), a2()]).then(function (values) {
  console.log(values[0], values[1], values[2]);
});

// 方法上的 const self 并发也必须独立
function Sock() {}
Sock.prototype.go = function (id) {
  const self = this;
  self.id = id;
  return Promise.resolve(1).then(function () {
    return self.id;
  });
};
const s = new Sock();
const t = new Sock();
Promise.all([s.go('S'), t.go('T')]).then(function (values) {
  console.log(values[0], values[1]);
});
