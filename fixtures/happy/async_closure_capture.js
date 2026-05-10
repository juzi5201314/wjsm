function makeCounter() {
  var count = 0;
  return async function() {
    await Promise.resolve(undefined);
    count = count + 1;
    return count;
  };
}
var counter = makeCounter();
counter().then(v => console.log(v));
counter().then(v => console.log(v));
counter().then(v => console.log(v));
