// Properties from prototype chain are not included.
function Parent() {}
Parent.prototype.inherited = "from-proto";

const child = new Parent();
child.own = "own-value";
console.log(JSON.stringify(child));
