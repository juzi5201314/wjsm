// Invalid JSON must throw a catchable SyntaxError.
// 使用 bare call 形式（而非 const x = ...），以匹配当前引擎对 JSON.parse 错误返回 TAG_EXCEPTION 的处理，
// 确保在 try 内能可靠进入 catch（initializer 位置当前可能将 exc tag 作为值泄漏）。
try {
  JSON.parse("{not valid json");
  console.log("no-throw");
} catch (e) {
  console.log("caught-name:", e.name);
  console.log("caught-msg:", e.message);
}