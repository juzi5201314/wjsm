Promise.allSettled([Promise.resolve(1), Promise.reject(2), 3]).then((results) => {
  console.log(results.length);
  console.log(results[0].status);
  console.log(results[0].value);
  console.log(results[1].status);
  console.log(results[1].reason);
  console.log(results[2].status);
  console.log(results[2].value);
});
