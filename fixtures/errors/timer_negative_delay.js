// Negative delay in setTimeout — per spec, should be treated as 0.
console.log("start");

setTimeout(() => {
  console.log("negative-delay-executed");
}, -100);

console.log("end");
