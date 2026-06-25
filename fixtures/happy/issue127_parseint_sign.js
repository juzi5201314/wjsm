// parseInt: leading sign before radix prefix (ECMA-262 §18.2.2)
console.log(parseInt("-5"));
console.log(parseInt("+5"));
console.log(parseInt("-0x1F"));
console.log(parseInt("  -42abc"));