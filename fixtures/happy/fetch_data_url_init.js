const resp = await fetch("data:text/plain,hello", {
  method: "POST",
  headers: { "x-test": "1" },
});
console.log("ok", resp.ok);
console.log("status", resp.status);
const body1 = await resp.text();
console.log("body", body1);

const r = new Request("data:text/plain,x", { method: "PUT", headers: { "y": "2" } });
console.log("req method", r.method);
console.log("req header", r.headers.get("y"));
