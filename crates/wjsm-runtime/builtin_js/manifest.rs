// Builtin JS 扩展 manifest。
//
// 顺序敏感：每次 snapshot 期 eval 按此顺序拼接为 ES module seed。
// 任一 entry 改动（文件名 / source）都会经 hash 进入 ABI hash external input，
// 触发 embedded snapshot 失效 + 重新 bake。
//
// P3.0 阶段：空 manifest（与 builtin JS bundle 引入前的字节级行为一致）。
// 未来如果引入 Web/Node API 的 JS 实现（Promise.try、structuredClone 等），
// 在此 append `(name, include_str!("xxx.js"))` 即可。
pub static BUILTIN_JS_FILES: &[(&str, &str)] = &[];
