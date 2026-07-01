eval("var s = \"let phantom\"; var {a: real, b: [nested]} = {a: 1, b: [2]}; function named() {} class ignored {}");
console.log(real);
console.log(nested);
console.log(typeof named);
console.log("phantom" in globalThis);
console.log("ignored" in globalThis);
