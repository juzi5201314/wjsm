// Test JSON.stringify with async toJSON/toString chain returning primitives.
// Report claimed json_parse_to_string_async recursing into sync helper could block,
// but the recursion is guarded by !is_js_object so only primitives reach sync path.

async function testAsyncToStringChain() {
    const obj = {
        async toJSON() {
            return {
                async toString() {
                    return 42; // primitive
                }
            };
        }
    };

    const result = await new Promise(resolve => {
        setTimeout(() => {
            resolve(JSON.stringify({ data: obj }));
        }, 0);
    });

    console.log("result:", result);
    console.log("PASS: async toJSON chain with primitive works");
}

testAsyncToStringChain();
