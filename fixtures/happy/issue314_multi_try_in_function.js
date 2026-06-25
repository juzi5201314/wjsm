function test() {
    const results = [];
    try { 1n + 2; results.push("add_fail"); } catch (e) { results.push("add:" + e.name); }
    try { 1n - 2; results.push("sub_fail"); } catch (e) { results.push("sub:" + e.name); }
    try { 1n * 2; results.push("mul_fail"); } catch (e) { results.push("mul:" + e.name); }
    try { 1n / 2; results.push("div_fail"); } catch (e) { results.push("div:" + e.name); }
    try { 1n % 2; results.push("mod_fail"); } catch (e) { results.push("mod:" + e.name); }
    try { 1n ** 2; results.push("exp_fail"); } catch (e) { results.push("exp:" + e.name); }
    try { results.push("add_bigint:" + (1n + 2n)); } catch (e) { results.push("add_bigint_fail"); }
    try { results.push("add_number:" + (1 + 2)); } catch (e) { results.push("add_number_fail"); }
    console.log(results.join(","));
}
test();