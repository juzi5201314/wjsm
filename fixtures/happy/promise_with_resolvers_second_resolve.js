const cap = Promise.withResolvers();
cap.resolve(1);
Promise.all([Promise.resolve(2)]).then(() => {
  cap.resolve(3);
  cap.promise.then((v) => console.log(v));
});