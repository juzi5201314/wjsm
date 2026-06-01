// JSON.parse 嵌套数组、对象和 null 解析。
const result = JSON.parse('[1,{"x":true},null]');
console.log("nested-result:", result);