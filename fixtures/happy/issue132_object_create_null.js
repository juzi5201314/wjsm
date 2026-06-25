const o = Object.create(null);
console.log("proto null:", Object.getPrototypeOf(o) === null);
console.log("has toString:", "toString" in o);