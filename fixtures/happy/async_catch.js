async function recover() {
  try {
    await Promise.reject("boom");
  } catch (e) {
    return "caught";
  }
}

recover().then(v => console.log(v));
