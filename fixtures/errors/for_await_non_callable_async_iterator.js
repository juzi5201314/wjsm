// for await...of with non-callable @@asyncIterator should throw TypeError
async function test() {
    const badIterable = {
        [Symbol.asyncIterator]: 123  // non-callable
    };
    try {
        for await (let x of badIterable) {
            console.log(x);
        }
        console.log("FAIL: did not throw");
    } catch (e) {
        console.log("PASS: caught TypeError");
    }
}
test();
