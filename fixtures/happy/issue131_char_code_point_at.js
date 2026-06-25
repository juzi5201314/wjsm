const s = "\u{1D400}";
console.log(s.charCodeAt(0).toString(16));
console.log(s.charCodeAt(1).toString(16));
console.log(s.codePointAt(0).toString(16));
console.log(s.codePointAt(1).toString(16));