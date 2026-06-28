var re = /\d/g;
console.log(re.lastIndex);
re.lastIndex = 2;
console.log(re.lastIndex);
var m = re.exec("ab3cd");
console.log(m ? m[0] : null, re.lastIndex);
re.lastIndex = 0;
var m2 = re.exec("ab3cd");
console.log(m2 ? m2[0] : null, re.lastIndex);
