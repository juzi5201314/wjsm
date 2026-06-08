let x;
try {
    eval('var r = x;');
    console.log("no_error");
} catch (e) {
    console.log("tdz_error");
}