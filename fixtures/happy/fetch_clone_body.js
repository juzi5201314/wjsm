// fetch_clone_body
// Request/Response .clone() must not deadlock on headers_table
// clones get independent headers and body state; bodyUsed starts false on clone
// consuming original must not affect clone and vice versa; consuming twice errors

(async () => {
  // Response from fetch (uses create_response_object path)
  const res = await fetch("data:text/plain,clonetest");
  console.log("orig bodyUsed before:", res.bodyUsed);
  const clone1 = res.clone();
  console.log("clone1 bodyUsed init:", clone1.bodyUsed);
  console.log("headers different objects:", res.headers !== clone1.headers);

  const t1 = await res.text();
  console.log("orig text len:", t1.length);
  console.log("orig bodyUsed after:", res.bodyUsed);
  console.log("clone still not used:", clone1.bodyUsed);

  const t2 = await clone1.text();
  console.log("clone text len:", t2.length);
  console.log("clone bodyUsed after its consume:", clone1.bodyUsed);

  // second consume on orig should reject
  try {
    await res.text();
    console.log("ERROR: second consume on orig succeeded");
  } catch (e) {
    console.log("second on orig error name:", e.name);
  }

  // Request clone
  const req = new Request("data:text/plain,reqclone", { method: "POST", body: "abc" });
  const reqClone = req.clone();
  console.log("req clone url same:", reqClone.url === req.url);
  console.log("req clone bodyUsed init:", reqClone.bodyUsed);

  console.log("clone body test done");
})();
