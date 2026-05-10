Promise.all([1, Promise.resolve(2), 3]).then(values => console.log(values[0] + values[1] + values[2]));
