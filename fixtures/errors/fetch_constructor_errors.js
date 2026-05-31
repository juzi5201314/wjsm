// fetch_constructor_errors
// invalid header name/value, Headers sequence bad length, Request bad URL/credentials/forbidden method/GET+body, Response bad status + body on null-body status

(async () => {
  // Headers invalid
  try {
    new Headers({ "Bad Name": "v" });
    console.log("bad header name no error");
  } catch (e) { console.log("bad header name:", e.name); }

  try {
    new Headers([["ok", "v"], ["bad\nname", "v"]]);
    console.log("bad header in seq no error");
  } catch (e) { console.log("bad header name seq:", e.name); }

  try {
    new Headers([["x", "v1", "extra"]]);  // length != 2
    console.log("seq bad len no error");
  } catch (e) { console.log("seq entry len:", e.name); }

  try {
    new Headers([["x", "bad\nvalue"]]);
    console.log("bad value no error");
  } catch (e) { console.log("bad header value:", e.name); }

  // Request bad
  try {
    new Request("http://user:pass@example.com");
    console.log("creds in url no error");
  } catch (e) { console.log("url creds:", e.name); }

  try {
    new Request("/rel", { method: "POST", body: "x" });  // but relative may be ok or not; use bad scheme?
  } catch (e) {}

  try {
    new Request("data:x", { method: "TRACE" });  // forbidden method?
    console.log("forbidden method no error");
  } catch (e) { console.log("forbidden method:", e.name); }

  try {
    new Request("data:x", { method: "GET", body: "should fail" });
    console.log("get with body no error");
  } catch (e) { console.log("get body:", e.name); }

  // Response bad status
  try {
    new Response(null, { status: 999 });
    console.log("bad status no error");
  } catch (e) { console.log("bad status:", e.name); }

  try {
    new Response("body", { status: 101 });  // switching protocols null body status?
    console.log("null body status with body no error");
  } catch (e) { console.log("status with body:", e.name); }

  try {
    new Response(null, { statusText: "bad\rtext" });
    console.log("bad statusText no error");
  } catch (e) { console.log("bad statusText:", e.name); }

  console.log("error fixture done");
})();
