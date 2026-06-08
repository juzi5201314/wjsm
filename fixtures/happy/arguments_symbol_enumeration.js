function f() {
    const keys = Object.keys(arguments);
    const symbols = Object.getOwnPropertySymbols(arguments);
    const ownKeys = Reflect.ownKeys(arguments);
    const foundIterator = ownKeys[3] === Symbol.iterator;
    console.log(keys.includes("Symbol.iterator"));
    console.log(keys.includes("length"));
    console.log(symbols.length);
    console.log(symbols[0] === Symbol.iterator);
    console.log(foundIterator);
}
f(1, 2);
