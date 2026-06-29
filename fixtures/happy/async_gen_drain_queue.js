// #141: async generator body 抛异常时，排队请求按 completion_type 分支
//   排队 Next -> fulfill {value: undefined, done: true}
async function* gthrow() { yield 7; throw new Error("body-boom"); }
const k = gthrow();
Promise.allSettled([k.next(), k.next(), k.next()]).then((rs) => {
  console.log("throw-drain", JSON.stringify(rs.map((r) =>
    r.status === "fulfilled" ? ["F", r.value.value, r.value.done] : ["R", String(r.reason)])));
});
// #142: 排队 Return 请求 fulfill 其自身的值（不丢失）
async function* gret() { yield "a"; yield "b"; }
const h = gret();
Promise.all([h.next(), h.next(), h.return(99)]).then((rs) => {
  console.log("return-drain", JSON.stringify(rs.map((r) => [r.value, r.done])));
});
