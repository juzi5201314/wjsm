class C {
  constructor() {
    this.x = 1;
  }
}

try {
  var c = new C();
  console.log("x=" + c.x);
} catch (e) {
  console.log("CATCH: " + e);
}

class Throws {
  constructor() {
    throw "boom";
  }
}

try {
  new Throws();
  console.log("unreachable");
} catch (e) {
  console.log("caught=" + e);
}

console.log("done");
