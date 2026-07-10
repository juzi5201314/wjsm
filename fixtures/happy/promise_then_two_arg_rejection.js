Promise.reject('cluster-error').then(
  () => console.log('unexpected-ok'),
  error => console.log('rejected', error),
);
Promise.resolve('cluster-ok').then(
  value => console.log('fulfilled', value),
  () => console.log('unexpected-error'),
);
Promise.resolve(1)
  .then(value => value + 1)
  .then(value => console.log('chained', value));
