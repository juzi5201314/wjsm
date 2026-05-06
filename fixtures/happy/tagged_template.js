function tag(strings, a, b) {
    return strings[0] + a + strings[1] + b + strings[2];
}
console.log(tag`Hello, ${'World'}! Goodbye, ${'Moon'}!`);
