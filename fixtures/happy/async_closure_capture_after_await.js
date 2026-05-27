async function demo() {
  let x = 41;
  let get = () => x + 1;
  await Promise.resolve(undefined);
  console.log(get());
}

demo();
