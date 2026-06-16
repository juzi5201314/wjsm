# ADR 0002: RuntimeState 保持扁平的侧表集合

## Status

Accepted

## Context

`RuntimeState`（`crates/wjsm-runtime/src/lib.rs`）持有约 50 个 `Arc<Mutex<…>>`
侧表字段（`promise_table`、`microtask_queue`、`map_table`、`set_table`、
`iterators`、`continuation_table`、各类 stream/fetch 表等）。架构审查中反复出现
一个提案：把这些字段按领域（Promise / Collection / Iterator / Stream …）分组
为子结构体，宣称能提升「可测试性」与「AI 可导航性」。

实测变更半径：promise 域约 60 处、collection 域约 56 处、iterator 域约 77 处
访问点，全仓 host 代码合计约 270 处形如 `caller.data().<table>` 的直接字段访问。

## Decision

**不**对 `RuntimeState` 做领域分组。侧表保持为 `RuntimeState` 上的扁平字段。

判定依据是 deletion test（想象删除该抽象，复杂度是「集中」还是「平移」）：

- **复杂度只是平移，未集中。** 分组把 `caller.data().map_table` 变成
  `caller.data().collections.map_table`，跨约 270 处访问点。接口复杂度（约 50 个
  字段）原样保留，只多了一层间接，杠杆（leverage）为零——属于浅层重组，不是
  深化（deepening）。

- **对可测试性无增益。** 每个 runtime 函数签名是
  `&mut Caller<'_, RuntimeState>`，而非某个子状态。仅分组字段不改变测试面
  （test surface）；要真正受益必须把每个函数重新穿线为接收收窄后的子状态，
  那是一次贯穿 runtime 的签名重写（数千行、高回归风险），远超提案隐含的范围。

- **原先设想的 locality 缺陷并不成立。** 当初担心「GC root 追踪必须知道每张表」。
  实际上该知识已集中在单一函数
  `runtime_gc/roots.rs::collect_host_table_values`——它一处枚举所有持 root 的
  侧表。新增持 root 的表时需要编辑这一处，这是已经存在的单点（locality 已达成），
  字段分组对它毫无帮助。

## Consequences

- `RuntimeState` 的扁平结构是刻意保留，不是技术债。未来架构审查不应把
  「按领域拆分 RuntimeState」当作深化机会重新提出，除非伴随把 runtime 函数签名
  收窄到子状态的完整方案（届时再评估其规模与回归风险）。
- 新增侧表时：在 `RuntimeState` 加字段、在 `RuntimeState::new` 初始化；若该表持有
  obj_table 引用，必须在 `runtime_gc/roots.rs::collect_host_table_values` 注册其
  root 快照，否则会漏标导致悬垂回收。这一条是真正的 locality 约束，与字段是否分组
  无关。
