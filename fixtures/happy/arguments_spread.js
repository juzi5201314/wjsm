function f(a, b, c) {
    const arr = [...arguments];
    console.log(arr.length);
    console.log(arr[0]);
    console.log(arr[1]);
    console.log(arr[2]);
}
f(1, 2, 3);
