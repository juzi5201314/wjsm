// #179: String.prototype.normalize (NFC default, NFD, NFKC)
var decomposed = "e\u0301";
var composed = "\u00e9";
console.log(decomposed.normalize() === composed.normalize());
console.log(decomposed.normalize("NFD") === decomposed);
console.log("\ufb00".normalize("NFKC") === "ff");