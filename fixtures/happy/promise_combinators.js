// 测试 Promise.all 基本功能
Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]).then(function(v) {
    console.log(v);
});

// 测试 Promise.race
Promise.race([Promise.resolve("first"), Promise.resolve("second")]).then(function(v) {
    console.log(v);
});

// 测试 Promise.allSettled
Promise.allSettled([Promise.resolve("ok"), Promise.reject("fail")]).then(function(v) {
    console.log(v[0].status);
    console.log(v[1].status);
});

// 测试 Promise.any
Promise.any([Promise.reject("err"), Promise.resolve("win")]).then(function(v) {
    console.log(v);
});
