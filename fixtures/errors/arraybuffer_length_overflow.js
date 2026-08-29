// ToIndex（§7.1.22）越界：2^53 超出 [0, 2^53-1]，按 V8 文案 RangeError
// 上抛（修复前该长度直达 vec![0; n] 触发宿主 allocator abort、退出码 134）。
new ArrayBuffer(9007199254740992);
