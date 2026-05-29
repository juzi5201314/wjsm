// Clear nonexistent timer id — must be silent, no crash.
console.log("start");

clearTimeout(999999);
clearTimeout(undefined);
clearTimeout(null);

console.log("end");
