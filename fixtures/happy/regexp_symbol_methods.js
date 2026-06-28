console.log("hello world".match(/o\w/g).join(","));
console.log("a-b-c".replace(/-/g, "+"));
console.log("foobar".search(/bar/));
console.log("1,2;3".split(/[,;]/).join("|"));
var custom = {
  [Symbol.match](s){ return ["M:" + s]; },
  [Symbol.replace](s, r){ return "R:" + s + ":" + r; },
  [Symbol.search](s){ return 42; },
  [Symbol.split](s){ return [s, s]; }
};
console.log("xx".match(custom)[0]);
console.log("yy".replace(custom, "Z"));
console.log("zz".search(custom));
console.log("ww".split(custom).join("|"));
