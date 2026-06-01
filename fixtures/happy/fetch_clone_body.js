// fetch_clone_body
// Request/Response .clone() must not deadlock on headers_table
// clones get independent headers and body state; bodyUsed starts false on clone
// consuming original must not affect clone and vice versa; consuming twice rejects

const res = new Response("clonetest");
console.log("orig bodyUsed before:", res.bodyUsed);
const clone1 = res.clone();
console.log("clone1 bodyUsed init:", clone1.bodyUsed);
console.log("headers different objects:", res.headers !== clone1.headers);

const req = new Request("data:text/plain,reqclone", { method: "POST", body: "abc" });
const reqClone = req.clone();
console.log("req clone url same:", reqClone.url === req.url);
console.log("req clone bodyUsed init:", reqClone.bodyUsed);

res.text().then(t1 => {
  console.log("orig text:", t1);
  console.log("orig bodyUsed after:", res.bodyUsed);
  console.log("clone bodyUsed after orig consume:", clone1.bodyUsed);

  clone1.text().then(t2 => {
    console.log("clone text:", t2);
    console.log("clone bodyUsed after its consume:", clone1.bodyUsed);

    res.text().then(undefined, e => {
      console.log("second on orig error name:", e.name);
      console.log("clone body test done");
    });
  });
});