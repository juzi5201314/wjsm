Promise.race([Promise.resolve("fast"), Promise.resolve("slow")]).then(function(v) {
  console.log(v);
});
