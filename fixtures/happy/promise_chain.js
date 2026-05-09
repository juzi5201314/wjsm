Promise.resolve(1).then(function(v) {
  return v + 1;
}).then(function(v) {
  console.log(v);
});
