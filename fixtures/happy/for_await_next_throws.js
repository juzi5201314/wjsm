// for await...of with an async iterator whose next() synchronously throws.
// The exception should propagate as rejected promise → await → catchable.

async function testAsyncIteratorThrows() {
    const badIterable = {
        [Symbol.asyncIterator]() {
            let count = 0;
            return {
                next() {
                    count++;
                    if (count === 2) {
                        throw new TypeError("async iterator threw at step 2");
                    }
                    return Promise.resolve({ value: count, done: false });
                }
            };
        }
    };

    try {
        let collected = [];
        for await (let x of badIterable) {
            collected.push(x);
        }
        console.log("FAIL: did not throw");
    } catch (e) {
        console.log("PASS: caught", e.name, "-", e.message);
    }
}

testAsyncIteratorThrows();
