const date = new Date(Date.UTC(2023, 0, 15, 12, 30, 45, 123));
console.log(
  date.getUTCFullYear(),
  date.getUTCMonth(),
  date.getUTCDate(),
  date.getUTCHours(),
  date.getUTCMinutes(),
  date.getUTCSeconds(),
  date.getUTCMilliseconds(),
);
console.log(
  typeof Date.prototype.getTime,
  Date.prototype.hasOwnProperty('getTime'),
  date.hasOwnProperty('getTime'),
);
console.log(Date.prototype.getTime.call(date));
try {
  Date.prototype.getTime.call({});
} catch (error) {
  console.log(error.name);
}
console.log(date.toISOString());
console.log(date.toUTCString());
console.log(date.setUTCFullYear(2024, 1, 29));
console.log(date.toISOString());
console.log(date.setUTCMonth(13, 1));
console.log(date.toISOString());
console.log(Date.parse('2023-01-15T12:30:45.123Z'));
console.log(Date.UTC(2023, 0, 15, 12, 30, 45, 123));
console.log(new Date('2023-01-15T12:30:45.123Z').getTime());
const invalid = new Date(NaN);
console.log(invalid.toString(), invalid.toJSON() === null);
try {
  invalid.toISOString();
} catch (error) {
  console.log(error.name);
}
