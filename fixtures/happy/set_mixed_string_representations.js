const literal = 'http';
const dynamic = ['ht', 'tp'].join('');
const literalSet = new Set([literal]);
const dynamicSet = new Set([dynamic]);

gc();

console.log(literalSet.has(dynamic));
console.log(dynamicSet.has(literal));
