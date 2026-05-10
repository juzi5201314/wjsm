Promise.any([Promise.reject(1), Promise.resolve(5)]).then((value) => console.log(value));
Promise.any([Promise.reject(1), Promise.reject(2)]).catch(() => console.log("rejected"));
