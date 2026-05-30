// Timer callbacks receive no implicit arguments.
setTimeout(function (value = 1) {
  console.log(arguments.length, value);
}, 0);
