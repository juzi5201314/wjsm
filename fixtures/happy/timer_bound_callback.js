// Bound timer callbacks preserve bound this and arguments.
const target = { value: 7 };
function callback(arg) {
  console.log(this.value, arg, arguments.length);
}
setTimeout(callback.bind(target, 3), 0);
