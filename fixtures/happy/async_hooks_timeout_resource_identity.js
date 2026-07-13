const t = setTimeout(function () {
  console.log('fired');
}, 0);
console.log(typeof t);
console.log(t.__brand__);
console.log(typeof t.__timer_id__);
