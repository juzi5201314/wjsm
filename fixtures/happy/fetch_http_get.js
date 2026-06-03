// KNOWN-NETWORK: requires HTTP access
fetch("https://httpbin.org/get")
  .then(r => {
    console.log(r.status);
    console.log(r.ok);
    return r.text();
  })
  .then(t => console.log(t.length > 0))
  .catch(e => console.log("error: " + e.message));
