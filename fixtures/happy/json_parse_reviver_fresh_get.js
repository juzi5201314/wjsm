const o = JSON.parse('{"a":1,"b":2}', function (k, v) {
  if (k === 'a') this.b = 99;
  return v;
});
console.log(o.b);