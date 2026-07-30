# 标识符、显示格式与稳定快照

这一章讲 `dump_text` 的输出格式为什么必须稳定，以及它作为快照测试基准的约束。

## dump_text 是 IR 的稳定序列化

`Module::dump_text()` 产出人类可读的 IR 文本，结构固定：

```text
module {
  constants:
    c0 = undefined
    c20 = number(1)

  fn @$module_main [entry=bb0]:
    bb0:
      %21 = const c20
      store var $0.x, %21
      return
}
```

常量池为空时打印 `constants: []`，非空时逐行 `c<N> = <Constant>`。函数之间空一行。

`Function::dump_text()` 单独 dump 一个函数，`wjsm dump-ir --func` 走这条路径，并附上常量块以便 `cN` 可解析。

## 函数头的标记

函数签名行会带上语义层填充的属性：

```text
fn @foo [needs_prototype] [params: $1.$env, $1.$this] [entry=bb0]:
fn @A.constructor [home_object=@0.prototype] [params: $2.$env, $2.$this] [entry=bb0]:
```

`captured_names` 非空时也会出现在头部。这些标记不是装饰，它们是快照对比的一部分：闭包捕获集合变化、`needs_prototype` 翻转都会体现在 diff 里。

## 快照测试依赖格式稳定

`fixtures/semantic/*.ir` 是 123 个 IR 快照，由 `crates/wjsm-semantic/tests/lowering_snapshots.rs` 逐个断言。每个测试是一个显式函数，调用 `assert_snapshot("<name>")`。

改动 lowering 会让快照失配。正确流程是先确认新 IR 语义正确，再更新：

```bash
cargo nextest run -p wjsm-semantic -- lowering_snapshots
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

更新后必须逐行审阅 diff。快照的价值在于让 lowering 的每次行为变化都可见，无脑重写等于放弃这层保护。

## 格式改动的代价

调整 `dump_text` 的输出格式（缩进、标记顺序、`Display` 实现）会让全部 123 个快照同时失配，而这些 diff 里没有任何语义信息。除非当前格式确实妨碍表达，不要动它；要动就在同一次提交里完成格式改动 + 快照重生成，不要与语义改动混在一起。

`ValueId` 等 ID 类型的 `Display` 实现（`%N`、`bbN`、`cN`、`modN`）是这个格式的一部分，同样受此约束。

> <details><summary>「快照测试」和「单元测试」的本质区别</summary>
>
> 单元测试断言「特定输入产生特定输出」——你写 `assert_eq!(func(x), expected)`，expected 是人工写的。
>
> 快照测试断言「输入产生与之前相同的输出」——你写 `assert_snapshot(name)`，expected 是「上次跑出来的结果」。
>
> 区别在于「预期」是谁定义的：
>
> - 单元测试：开发者基于理解写预期。理解错了，测试跟着错——但只要理解不变，测试也稳定。
> - 快照测试：开发者基于「现状」接受预期。改任何东西都让快照失灵——但这正是它的价值。
>
> 快照测试适合「我不在乎输出长什么样，只在乎它没变」的场景：渲染输出、序列化格式、IR dump、HTML 模板。这类东西人工写 expected 太繁琐、容易漏掉细节。代码一变就更新快照——改动是「我接受这个变化」的明确信号。
>
> 不适合：业务逻辑。业务逻辑的预期应该是显式的（`assert_eq!(total, 42)`），而不是「上次跑出来是 42」。
>
> wjsm 的 IR 快照是教科书式的快照测试用法：IR 是中间表示，diff 里只有结构变化，没有人眼能直接验证的「正确性」——但「这次结构变了」是真实信号，强制开发者审视。
>
> </details>

## 深入了解

- [语义 IR 快照的完整测试机制](../testing/semantic-snapshots.md)
- [dump-ir 命令的实现与过滤逻辑](../tooling/dump-and-disassembly.md)
- [`wjsm dump-ir` 的用户侧用法](../../user/cli/dump-ir.md)
