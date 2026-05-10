// 测试 Promise.resolve 对已有 Promise 的身份检查
var p = Promise.resolve(42);
var p2 = Promise.resolve(p);
console.log(p === p2);
p2.then(function(v) { console.log(v); });

// 测试 Promise.reject 创建新的 rejected promise
var p3 = Promise.reject("err");
p3.then(
    function(v) { console.log(v); },
    function(e) { console.log(e); }
);
