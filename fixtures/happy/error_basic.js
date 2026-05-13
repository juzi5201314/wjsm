var e = new Error("test message");
console.log(e.message);
console.log(e.name);
console.log(e.toString());

var e2 = new Error();
console.log(e2.message);
console.log(e2.name);

var e3 = new Error(undefined);
console.log(e3.message);
