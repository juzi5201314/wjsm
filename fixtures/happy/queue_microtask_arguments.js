// queueMicrotask callbacks receive no implicit arguments.
queueMicrotask(function (value = 1) {
  console.log(arguments.length, value);
});
