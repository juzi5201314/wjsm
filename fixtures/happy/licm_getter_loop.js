// LICM 语义回归：accessor 属性读取有副作用，不得外提。
// 每迭代 getter 自增：sum = 1 + 2 + 3 = 6。若被外提，sum 恒为 3。
let n = 0;
const P = {
  get x() {
    n = n + 1;
    return n;
  },
};
let sum = 0;
for (let i = 0; i < 3; i++) {
  sum = sum + P.x;
}
console.log(sum);
console.log(n);
