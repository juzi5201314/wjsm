// 测试 finally 保留原始值
Promise.resolve(42).finally(function() {}).then(function(v) {
    console.log(v);
});

// 测试 finally 保留原始拒绝原因
Promise.reject("err").finally(function() {}).then(
    function(v) { console.log(v); },
    function(e) { console.log(e); }
);
