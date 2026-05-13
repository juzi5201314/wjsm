var te = new TypeError("type error msg");
console.log(te.name);
console.log(te.message);
console.log(te.toString());

var re = new RangeError("range error msg");
console.log(re.name);
console.log(re.message);
console.log(re.toString());

var se = new SyntaxError("syntax error msg");
console.log(se.name);
console.log(se.toString());

var ref = new ReferenceError("ref error msg");
console.log(ref.name);
console.log(ref.toString());

var uri = new URIError("uri error msg");
console.log(uri.name);
console.log(uri.toString());

var ev = new EvalError("eval error msg");
console.log(ev.name);
console.log(ev.toString());
