// for await...of with sync iterator (async-from-sync) whose next() throws.
async function testSyncIteratorThrows() {
    const badIterable = {
        [Symbol.iterator]() {
            let count = 0;
            return {
                next() {
                    count++;
                    if (count === 2) {
                        throw new TypeError("sync iterator threw at step 2");
                    }
                    return { value: count, done: false };
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

testSyncIteratorThrows();