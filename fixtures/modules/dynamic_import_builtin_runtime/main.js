// 运行时路径（非静态说明符）动态 import 内置模块：宿主 DynamicImportRuntime
// 必须返回 promise 并兑现真实命名空间，解析/加载失败以 rejection 传播。
// 两次 import 用 then 链定序——跨 import 的结算先后是宿主加载细节，不可断言。
const builtinSpecifier = "node:" + "url";
import(builtinSpecifier).then((ns) => {
  console.log(typeof ns.fileURLToPath);
  console.log(typeof ns.pathToFileURL);
  console.log(typeof ns.URL);
  return import("./missing_" + "module.mjs").then(
    () => console.log("unexpected fulfill"),
    (error) => console.log("rejected", error instanceof Error),
  );
});
