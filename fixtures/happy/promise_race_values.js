Promise.race([7, Promise.resolve(9)]).then((value) => console.log(value));
Promise.race([Promise.reject(4), Promise.resolve(9)]).catch((reason) => console.log(reason));
