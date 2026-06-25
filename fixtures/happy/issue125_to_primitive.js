// ECMAScript §7.1.1 ToPrimitive (hint Number / String / @@toPrimitive)
console.log(+{ valueOf() { return 5; } });
console.log(`${{ toString() { return "x"; } }}`);
console.log(
  (function () {
    const o = {
      [Symbol.toPrimitive](hint) {
        return hint === "number" ? 7 : "s";
      },
    };
    return +o;
  })()
);