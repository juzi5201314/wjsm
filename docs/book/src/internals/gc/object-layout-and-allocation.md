# 对象布局与分配

本章详细讲解 wjsm 对象分配、table 布局与内存管理下的对象模型，对应 GC 侧定制和核心性能演化。

## HandleTableV2 设计

所有 JS 对象均以 handle index 访问，不暴露指针/地址，保证 GC 移动安全。`HandleTableV2` 按类型划分 slot，类型号内联在 NaN-box 值的负载中。

slot 可存 Object/Array/Function/String/BigInt/RegExp 等多类对象，类型区分见 value-representation.md。

## 分配与回收

对象通过 `gc_{flavor}::new_object` 分配 slot，释放交给 GC。handle 不复用，保证生命周期一致性。GC algos 区别在于 region/mark/sweep 细节，接口一致。

## 属性表与原型链

对象存储属性表，采用哈希表实现，属性 lookup 路径按原型链递归查找。性能关键路径用 SIMD 优化（可选），碰撞链长度短于 8。

- 普通对象：kv 哈希表
- Array：length 属性特殊处理，存储稀疏
- Function：env/call属性，闭包专用 slot

## 多 GC flavor 兼容

HandleTableV2 支持三种 GC flavor。内部分配器只关心 slot 分布与布局，GC 只需保证回收时正确擦除引用。

> <details><summary>为什么不用直接指针？</summary>
>
> 同前章所述，移动安全+NaN-box值承诺内存安全。裸指针部署下游诱发 UAF、悬垂错误。
>
> </details>

## 深入了解

- [GC 侧的句柄管理与 slot 组织](README.md)
- [值类型分布与 NaN-box 布局](../backend/value-representation.md)
- [数组、闭包、函数等具体类型布局](../backend/functions-closures-and-table.md)
