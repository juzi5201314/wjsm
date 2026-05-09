Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]).then(function(v) {
  console.log(v[0] + v[1] + v[2]);
});
