let calls = 0;
new Promise((resolve, reject) => {
  resolve(1);
  reject(2);
  resolve(3);
}).then(v => {
  calls = calls + 1;
  console.log(v);
});
Promise.resolve().then(() => console.log(calls));
