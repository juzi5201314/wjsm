let thenable = {
  then: resolve => resolve(7)
};

Promise.resolve(0)
  .then(() => thenable)
  .then(value => console.log(value));
