# size

按 WebAssembly section 拆分显示 `.wasm` 文件的体积构成。

```bash
wjsm size app.wasm
```

```text
WASM Size Breakdown for app.wasm
──────────────────────────────────────────────────
Section              Bytes    % Total
──────────────────────────────────────────────────
Type                   240       0.9%
Import               12204      47.6%
Export               10343      40.3%
Data                   607       2.4%
Code                  2112       8.2%
──────────────────────────────────────────────────
Total                25641
File Size            25686
```

`Total` 是各 section 字节数之和，`File Size` 是文件实际大小，差值来自模块头等不计入 section 的字节。

小程序的体积几乎全在 `Import` 和 `Export`：wjsm 编译出的模块要声明全部宿主函数 import 和运行时需要的 export，这部分是固定开销，不随你的代码增长。`Code` 才是你的代码本身，`Data` 是字符串等常量数据段。判断「我的代码有多大」时看 `Code` 和 `Data`，不要看总量。

同一个 section 名可能出现多次（例如多个 `Custom`），它们分别列出而不是合并。

> <details><summary>体积分析告诉你什么、不告诉你什么</summary>
>
> `size` 告诉你「现在产物由哪几部分组成、每部分多大」。这能回答：「为什么我的 .wasm 有 25 KB？」——`Import` 12 KB，`Export` 10 KB，两者加起来。
>
> 但它不直接告诉你「怎么变小」。几条常见路径：
>
> - **减少 `Code` 段**：精简业务代码、移除未使用函数。`Code` 段大致和源代码行数成正比。
> - **减少 `Data` 段**：合并重复字符串、把长字面量抽到文件。`Data` 段是字符串字面量直接编码进 WASM。
> - **Import/Export 段动不了**：这是 wjsm 的固定成本，源代码优化对它没影响。
>
> 想要 CI 体积门禁：在 build 后跑 `wjsm size dist/app.wasm | grep Code`，把 `Code` 段字节数提取出来比较。变化超过阈值就失败。
>
> </details>

## 深入了解

- [WASM 校验与尺寸分析的实现](../../internals/tooling/validation-and-size.md)
- [字符串、常量与数据段布局](../../internals/backend/strings-constants-and-data.md)
