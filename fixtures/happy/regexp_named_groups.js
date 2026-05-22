var re = /(?<year>\d{4})-(?<month>\d{2})-(?<day>\d{2})/;
var m = re.exec("2026-05-22");
console.log(m[0]);
console.log(m[1]);
console.log(m[2]);
console.log(m[3]);
console.log(JSON.stringify(m.groups));
