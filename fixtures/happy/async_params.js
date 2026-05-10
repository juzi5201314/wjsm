async function add(a, b) {
  const left = await Promise.resolve(a);
  const right = await Promise.resolve(b);
  return left + right;
}

add(3, 4).then(v => console.log(v));
