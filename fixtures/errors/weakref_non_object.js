try {
    new WeakRef(42);
    console.log('no error');
} catch (e) {
    console.log('error caught');
}
