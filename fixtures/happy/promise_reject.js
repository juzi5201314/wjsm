let p = new Promise(function(resolve, reject) {
  reject("error");
});
p.catch(function(e) {
  console.log(e);
});
