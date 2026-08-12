# Workspace crate 地图

当前 workspace 的生产职责如下；精确成员集合以 `cargo metadata --format-version 1` 为准。

| Crate | 职责 |
| --- | --- |
| `wjsm-parser` | SWC 解析与源码诊断 |
| `wjsm-semantic` | scope/early-error/lowering |
| `wjsm-ir` | 后端无关 semantic IR、value/builtin wire IDs |
| `wjsm-module` | module graph、ESM/CJS resolution 与 bundling |
| `wjsm-artifact-format` | portable `.wjsm` encode/decode/limits/hash/verify |
| `wjsm-native-abi` | vmctx、CallArgs、frames、symbols 与 native ABI hash |
| `wjsm-backend-native` | CLIF lowering、object、relocation、W^X、unwind、image/cache |
| `wjsm-host-native` | NativeRuntime、host dispatch、scheduler、modules、snapshot、inspector |
| `wjsm-host` | backend-independent host/ExecContext contract |
| `wjsm-builtins` | ECMAScript/WHATWG/Node semantic algorithms |
| `wjsm-gc` | ManagedHeap、HandleTableV2、collectors与虚拟内存 |
| `wjsm-runtime` | native runtime public facade，只 re-export |
| `wjsm-cli` | CLI/config/input/artifact/run orchestration |
| `wjsm-test262` | Test262 runner |
| `wjsm-gc-bench` | 三 collector benchmark |
| `wjsm-bench` | Node/native benchmark harness |

## 依赖边界

Cranelift、object、平台映射与 native ABI 只属于 `wjsm-backend-native`/`wjsm-host-native`。`wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 不依赖执行 backend。portable artifact 不依赖 native image/cache。

## 修改落点

| 改动 | Owner |
| --- | --- |
| 新语法/early error | `wjsm-semantic`，必要时 `wjsm-ir` |
| IR wire/layout | `wjsm-ir` + artifact verifier + semantic snapshots |
| module resolution | `wjsm-module` |
| artifact schema/limits | `wjsm-artifact-format` |
| CLIF/relocation/unwind | `wjsm-backend-native` |
| JS algorithm | `wjsm-builtins` / `wjsm-host` |
| runtime object/Promise/I/O | `wjsm-host-native` |
| heap/handle/barrier/GC | `wjsm-gc` |
| public CLI | `wjsm-cli` |

不得为同一语义新增 parallel owner、fallback 或旧 backend alias。
