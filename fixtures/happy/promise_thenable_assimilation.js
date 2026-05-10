let thenable = {
  then: resolve => resolve(3)
};
Promise.resolve(thenable).then(v => console.log(v));
