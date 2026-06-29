var it = [10, 20][Symbol.iterator]();
console.log(it.next().value);
console.log(it[Symbol.iterator]() === it);
console.log([...[30, 40][Symbol.iterator]()].join(","));
console.log(Array.from([50, 60][Symbol.iterator]()).join(","));
