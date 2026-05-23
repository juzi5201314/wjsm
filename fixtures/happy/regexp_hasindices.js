var re = /(?<x>a)(b)/d;
var m = re.exec("ab");
console.log(m.indices[0][0], m.indices[0][1]);
console.log(m.indices[1][0], m.indices[1][1]);
console.log(m.indices[2][0], m.indices[2][1]);
console.log(JSON.stringify(m.indices.groups));
