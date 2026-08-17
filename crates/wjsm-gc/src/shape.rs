//! 隐藏类（Shape）表：对象属性元数据的唯一宿主侧 owner。
//!
//! # 为什么属性元数据不在堆里
//!
//! 重构前每个属性占 32 字节堆内槽（name_id | flags | value | getter | setter），
//! 属性查找是线性扫 name_id；旧实现只能展开固定槽数，属性多了就落宿主。
//! 现在堆内只留**紧凑值数组**（每槽 8 字节 boxed i64，与数组元素完全同构），
//! name_id / flags / 值槽下标全部搬到本模块的 [`ShapeTable`]：
//!
//! ```text
//! 对象堆布局（16 字节 header + N×8 值槽）
//! +0   u32  proto handle      ← 与数组共用，GC / handle remap 的唯一 proto 来源
//! +4   u8   heap_type
//! +5   3B   pad
//! +8   u32  value_capacity    ← 值槽容量（不是属性数）
//! +12  u32  shape_id          ← 指向 ShapeTable
//! +16  8×N  值槽（boxed i64）
//! ```
//!
//! 这带来三个直接后果：
//!
//! 1. **属性读退化为「u32 shape 比较 + 常量偏移 load」**，不需要遍历任何 shape
//!    结构（IC 槽里存的是编译期无关的 `shape_id` + `value_index`）。
//! 2. **GC / handle remap / ZGC 重定位 / 快照恢复统统按 `24 + capacity*8` 走**，
//!    每槽当作一个 boxed i64，与数组元素同一套公式——不再需要「每槽 trace 三个字
//!    并按 flags 区分 accessor」的分支。未使用的值槽恒为 0（即 `+0.0`），不是句柄，
//!    因此扫描整个 capacity 而非 `slot_count` 也是安全的，扫描期无需查 ShapeTable。
//! 3. **accessor 属性占两个相邻值槽**（`index` = getter，`index + 1` = setter），
//!    没有独立侧表，因此上面第 2 点的同构性对 accessor 同样成立。
//!
//! # Shape 共享与退化
//!
//! `{a:1,b:2}` 与 `{a:3,b:4}` 经 `transition_add` 的 transition 缓存命中同一
//! shape_id，这是 inline cache 能命中的前提。三种情况退化为**字典 shape**
//! （每个对象独占、永不共享、IC 永不回填）：
//!
//! - 属性数超过 [`DICTIONARY_THRESHOLD`]
//! - 发生 `delete`（下标会出现空洞，无法维持「transition 只追加」的不变量）
//! - shape 表总数超过 [`SHAPE_TABLE_BUDGET`]（全局内存预算，正常程序不会触及）
//!
//! 字典 shape 仍然是一条正常的 `ShapeTable` 记录，属性存取路径与共享 shape 完全一致
//! ——**只有一套属性存储机制**，没有平行的字典实现。
//!
//! # 原型链 IC 的失效协议
//!
//! IC 命中的前提是 `obj_shape == ic_shape` 精确相等，所以「接收者自身」的任何形状
//! 变化会自动使 IC 失效。但原型链上的属性（`kind=ProtoData/Accessor`）
//! 还要防住两件事：接收者的 proto 被换掉、以及链中任意一环长出遮蔽属性。
//!
//! 本模块用一个**全局 proto 世代计数器**覆盖这两种情况：凡是被当作某个对象原型的
//! 句柄（[`ShapeTable::note_prototype`] 登记）发生形状变化，就 bump
//! [`ShapeTable::proto_generation`]；`kind=ProtoData/Accessor` 的 IC 槽记录填充时
//! 的世代，命中要求世代相等。原型在 bootstrap 之后极少长属性，所以 bump 罕见；
//! 一次 bump 让所有原型链 IC 一起重新预热，代价远低于逐槽反向依赖表。
//!
//! proto **句柄本身**换掉（`o.__proto__ = x`）由 IC 槽内缓存的 `expected_proto`
//! 比较覆盖，不需要 bump 世代。

use std::collections::{HashMap, HashSet};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use wjsm_ir::constants::{
    DICTIONARY_THRESHOLD, FLAG_IS_ACCESSOR, SHAPE_ID_EMPTY, SHAPE_MAP_THRESHOLD, SHAPE_TABLE_BUDGET,
};

/// 单个属性在 shape 中的描述。`index` 是**值槽下标**，不是属性序号：
/// 数据属性占 1 槽，accessor 属性占 `index` / `index + 1` 两槽。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShapeProp {
    pub name_id: u32,
    pub flags: u32,
    pub index: u32,
}

impl ShapeProp {
    /// 该属性是 accessor（值槽里存 getter/setter 而非数据值）。
    pub const fn is_accessor(&self) -> bool {
        self.flags & FLAG_IS_ACCESSOR as u32 != 0
    }

    /// 该属性占用的值槽数。
    pub const fn slot_span(&self) -> u32 {
        slot_span_for(self.flags)
    }

    /// getter 值槽下标（accessor 专用）。
    pub const fn getter_index(&self) -> u32 {
        self.index
    }

    /// setter 值槽下标（accessor 专用）。
    pub const fn setter_index(&self) -> u32 {
        self.index + 1
    }
}

const fn slot_span_for(flags: u32) -> u32 {
    if flags & FLAG_IS_ACCESSOR as u32 != 0 {
        2
    } else {
        1
    }
}

/// `transition_add` / `update_flags` 的结果：目标 shape、属性值槽下标、
/// 目标 shape 需要的值槽总数（调用方据此确保对象容量足够）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShapeTransition {
    pub shape_id: u32,
    pub index: u32,
    pub slot_count: u32,
    /// 属性种类（数据↔accessor）发生改变时，旧值槽被弃用；调用方应把
    /// `[abandoned, abandoned + span)` 清零，避免死槽让 GC 误留对象存活。
    pub abandoned: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Shape {
    /// 按插入序排列；枚举顺序即此顺序。
    props: Vec<ShapeProp>,
    /// name_id → `props` 下标；`props.len() >= SHAPE_MAP_THRESHOLD` 时建立。
    #[serde(skip)]
    index_of: Option<HashMap<u32, u32>>,
    /// `(name_id, flags)` → 目标 shape_id。字典 shape 恒空（不共享）。
    #[serde(skip)]
    transitions: HashMap<(u32, u32), u32>,
    /// 本 shape 需要的值槽总数。
    slot_count: u32,
    /// 字典 shape：被单个对象独占，IC 永不回填。
    dictionary: bool,
}

impl Shape {
    fn find(&self, name_id: u32) -> Option<usize> {
        match &self.index_of {
            Some(map) => map.get(&name_id).map(|slot| *slot as usize),
            None => self.props.iter().position(|prop| prop.name_id == name_id),
        }
    }

    fn push_prop(&mut self, prop: ShapeProp) {
        if let Some(map) = &mut self.index_of {
            map.insert(prop.name_id, self.props.len() as u32);
        }
        self.props.push(prop);
        if self.index_of.is_none() && self.props.len() >= SHAPE_MAP_THRESHOLD as usize {
            self.rebuild_index();
        }
    }

    fn rebuild_index(&mut self) {
        self.index_of = Some(
            self.props
                .iter()
                .enumerate()
                .map(|(slot, prop)| (prop.name_id, slot as u32))
                .collect(),
        );
    }
}

/// 序列化形态：只导出属性结构与 transition 边，`index_of` 是可重建的查找加速表。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShapeTableSnapshot {
    shapes: Vec<Shape>,
    /// `(parent_shape_id, name_id, flags, child_shape_id)`，保证恢复后 shape 共享不退化。
    transitions: Vec<(u32, u32, u32, u32)>,
    prototypes: Vec<u32>,
    proto_generation: u32,
}

#[derive(Debug, Default)]
struct ShapeTableInner {
    /// 下标即 shape_id；0 号恒为空对象 shape。
    shapes: Vec<Shape>,
    /// 被当作某对象原型的句柄集合；其形状变化会 bump `proto_generation`。
    prototypes: HashSet<u32>,
    proto_generation: u32,
}

/// 隐藏类表。经 `&self` 提供内部可变性，因为 `HeapAccessV2` 以 `Arc` 跨宿主调用共享。
#[derive(Debug)]
pub struct ShapeTable {
    inner: RwLock<ShapeTableInner>,
}

impl Default for ShapeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeTable {
    pub fn new() -> Self {
        let mut shapes = Vec::with_capacity(64);
        shapes.push(Shape::default());
        Self {
            inner: RwLock::new(ShapeTableInner {
                shapes,
                prototypes: HashSet::new(),
                proto_generation: 0,
            }),
        }
    }

    /// 空对象 shape（无属性、值槽数 0）。
    pub const fn empty_shape() -> u32 {
        SHAPE_ID_EMPTY
    }

    pub fn shape_count(&self) -> u32 {
        self.inner.read().shapes.len() as u32
    }

    /// 查属性；命中返回 flags 与值槽下标。这是慢路径（IC 未命中）的查找入口。
    pub fn lookup(&self, shape_id: u32, name_id: u32) -> Option<ShapeProp> {
        let inner = self.inner.read();
        let shape = inner.shapes.get(shape_id as usize)?;
        shape.find(name_id).map(|slot| shape.props[slot])
    }

    /// 该 shape 需要的值槽总数。
    pub fn slot_count(&self, shape_id: u32) -> u32 {
        self.inner
            .read()
            .shapes
            .get(shape_id as usize)
            .map_or(0, |shape| shape.slot_count)
    }

    /// 属性数量（不是值槽数）。
    pub fn prop_count(&self, shape_id: u32) -> u32 {
        self.inner
            .read()
            .shapes
            .get(shape_id as usize)
            .map_or(0, |shape| shape.props.len() as u32)
    }

    /// 字典 shape 不参与共享，IC 永不回填。
    pub fn is_dictionary(&self, shape_id: u32) -> bool {
        self.inner
            .read()
            .shapes
            .get(shape_id as usize)
            .is_some_and(|shape| shape.dictionary)
    }

    /// 按插入序快照属性列表，供 `Object.keys` 等枚举路径使用。
    pub fn props(&self, shape_id: u32) -> Vec<ShapeProp> {
        self.inner
            .read()
            .shapes
            .get(shape_id as usize)
            .map_or_else(Vec::new, |shape| shape.props.clone())
    }

    /// 整张表当前所有 shape 的 name_id 并集。
    ///
    /// 宿主侧 string intern 表回收用它钉扎「曾作为属性名出现」的 name_id：
    /// 这些 id 即便对应的对象已死，仍可能被存活 shape 的 transition 引用，复用
    /// 会把后续属性定义别名到错误的属性名。
    pub fn all_name_ids(&self) -> HashSet<u32> {
        let inner = self.inner.read();
        inner
            .shapes
            .iter()
            .flat_map(|shape| shape.props.iter().map(|prop| prop.name_id))
            .collect()
    }

    /// 定义/覆盖属性 `name_id` 为 `flags`，返回目标 shape 与值槽下标。
    ///
    /// - 属性已存在且 flags 相同 → 原地返回，无 transition。
    /// - 属性已存在但 flags 不同 → 转移到「同名不同 flags」的 shape；若属性种类
    ///   （数据 ↔ accessor）改变则分配新值槽并在 `abandoned` 报出旧槽。
    /// - 属性不存在 → 追加。出边过多或属性过多则退化为字典 shape。
    pub fn transition_add(&self, shape_id: u32, name_id: u32, flags: u32) -> ShapeTransition {
        let mut inner = self.inner.write();
        inner.transition(shape_id, name_id, flags)
    }

    /// 收紧已存在属性的 flags（`Object.freeze` / `seal` / 描述符收紧）。
    /// 属性不存在时返回 `None`。
    pub fn update_flags(&self, shape_id: u32, name_id: u32, flags: u32) -> Option<ShapeTransition> {
        let mut inner = self.inner.write();
        let shape = inner.shapes.get(shape_id as usize)?;
        shape.find(name_id)?;
        Some(inner.transition(shape_id, name_id, flags))
    }

    /// 删除属性：对象退化为字典 shape，返回 `(新 shape_id, 被释放的值槽区间)`。
    /// 属性不存在时返回 `None`（调用方语义上仍算删除成功）。
    pub fn remove_prop(&self, shape_id: u32, name_id: u32) -> Option<(u32, (u32, u32))> {
        let mut inner = self.inner.write();
        let dictionary_id = inner.to_dictionary(shape_id);
        let shape = inner.shapes.get_mut(dictionary_id as usize)?;
        let slot = shape.find(name_id)?;
        let prop = shape.props.remove(slot);
        shape.rebuild_index();
        // 值槽不回收：保持其余属性下标稳定（IC 与已发射代码依赖下标不变）。
        Some((dictionary_id, (prop.index, prop.slot_span())))
    }

    /// 强制把对象转为字典 shape（`Object.setPrototypeOf` 之外的去优化入口）。
    pub fn to_dictionary(&self, shape_id: u32) -> u32 {
        self.inner.write().to_dictionary(shape_id)
    }

    // ── 原型链 IC 失效协议 ──────────────────────────────────────────────────

    /// 登记 `handle` 被用作某个对象的原型。此后它的形状变化会 bump proto 世代。
    pub fn note_prototype(&self, handle: u32) {
        if handle == PROTO_NULL_SENTINEL {
            return;
        }
        let mut inner = self.inner.write();
        inner.prototypes.insert(handle);
    }

    /// 当前 proto 世代；`kind=ProtoData/Accessor` 的 IC 槽命中要求世代相等。
    pub fn proto_generation(&self) -> u32 {
        self.inner.read().proto_generation
    }

    /// `handle` 若是原型则 bump proto 世代，使所有原型链 IC 失效。
    /// 对象形状发生任何变化时都必须调用。
    pub fn invalidate_if_prototype(&self, handle: u32) {
        let mut inner = self.inner.write();
        if inner.prototypes.contains(&handle) {
            inner.proto_generation = inner.proto_generation.wrapping_add(1);
        }
    }

    // ── 快照 / realm 克隆 ───────────────────────────────────────────────────

    /// 导出可序列化形态（startup snapshot / realm 克隆）。
    pub fn export(&self) -> ShapeTableSnapshot {
        let inner = self.inner.read();
        let mut transitions = Vec::new();
        for (parent, shape) in inner.shapes.iter().enumerate() {
            for ((name_id, flags), child) in &shape.transitions {
                transitions.push((parent as u32, *name_id, *flags, *child));
            }
        }
        transitions.sort_unstable();
        let mut prototypes: Vec<u32> = inner.prototypes.iter().copied().collect();
        prototypes.sort_unstable();
        ShapeTableSnapshot {
            shapes: inner.shapes.clone(),
            transitions,
            prototypes,
            proto_generation: inner.proto_generation,
        }
    }

    /// 从快照整体替换表内容。
    pub fn import(&self, snapshot: ShapeTableSnapshot) {
        let ShapeTableSnapshot {
            mut shapes,
            transitions,
            prototypes,
            proto_generation,
        } = snapshot;
        if shapes.is_empty() {
            shapes.push(Shape::default());
        }
        for shape in &mut shapes {
            shape.transitions.clear();
            if shape.props.len() >= SHAPE_MAP_THRESHOLD as usize {
                shape.rebuild_index();
            } else {
                shape.index_of = None;
            }
        }
        for (parent, name_id, flags, child) in transitions {
            if let Some(shape) = shapes.get_mut(parent as usize)
                && !shape.dictionary
            {
                shape.transitions.insert((name_id, flags), child);
            }
        }
        let mut inner = self.inner.write();
        inner.shapes = shapes;
        inner.prototypes = prototypes.into_iter().collect();
        inner.proto_generation = proto_generation;
    }

    /// realm 克隆：按 handle map 重写原型登记集合（shape 结构本身与句柄无关）。
    pub fn remap_prototypes(&self, remap: impl Fn(u32) -> u32) {
        let mut inner = self.inner.write();
        inner.prototypes = inner.prototypes.iter().map(|h| remap(*h)).collect();
    }
}

/// `+0` 处 proto 字段的 null 哨兵，与 `heap_access` / `object_walker` 共用。
pub const PROTO_NULL_SENTINEL: u32 = 0xFFFF_FFFF;

impl ShapeTableInner {
    fn transition(&mut self, shape_id: u32, name_id: u32, flags: u32) -> ShapeTransition {
        let Some(shape) = self.shapes.get(shape_id as usize) else {
            // 未知 shape_id 只可能来自损坏的堆；回落到空 shape 追加，语义等价于新对象。
            return self.transition(SHAPE_ID_EMPTY, name_id, flags);
        };

        // 字典 shape 独占于单个对象，原地改。
        if shape.dictionary {
            return self.transition_in_dictionary(shape_id, name_id, flags);
        }

        // 已存在且 flags 完全相同 → 无形状变化。
        if let Some(slot) = shape.find(name_id) {
            let prop = shape.props[slot];
            if prop.flags == flags {
                return ShapeTransition {
                    shape_id,
                    index: prop.index,
                    slot_count: shape.slot_count,
                    abandoned: None,
                };
            }
        }

        if let Some(target) = shape.transitions.get(&(name_id, flags)).copied() {
            let existing = shape.find(name_id).map(|slot| shape.props[slot]);
            let target_shape = &self.shapes[target as usize];
            let prop = target_shape.props[target_shape
                .find(name_id)
                .expect("transition target must carry the transitioned property")];
            let abandoned = existing
                .filter(|old| old.index != prop.index)
                .map(|old| (old.index, old.slot_span()));
            return ShapeTransition {
                shape_id: target,
                index: prop.index,
                slot_count: target_shape.slot_count,
                abandoned,
            };
        }

        // 属性过多，或 shape 表已超全局预算 → 退化字典。
        //
        // 刻意不按「单个 shape 的出边数」设限：空 shape 是整棵 transition 树的根，
        // 其出边数 = 全程序中作为首个属性出现过的名字个数（bootstrap 就有数百个）。
        // 按出边设限会让根 shape 迅速触顶，此后每个新对象都退化字典、IC 永久失效。
        if shape.props.len() >= DICTIONARY_THRESHOLD as usize
            || self.shapes.len() >= SHAPE_TABLE_BUDGET as usize
        {
            let dictionary_id = self.to_dictionary(shape_id);
            return self.transition_in_dictionary(dictionary_id, name_id, flags);
        }

        let mut target = shape.clone();
        target.transitions.clear();
        let existing = target.find(name_id).map(|slot| (slot, target.props[slot]));
        let span = slot_span_for(flags);
        let (index, abandoned) = match existing {
            Some((slot, old)) if old.slot_span() == span => {
                target.props[slot].flags = flags;
                (old.index, None)
            }
            Some((slot, old)) => {
                let index = target.slot_count;
                target.slot_count += span;
                target.props[slot] = ShapeProp {
                    name_id,
                    flags,
                    index,
                };
                (index, Some((old.index, old.slot_span())))
            }
            None => {
                let index = target.slot_count;
                target.slot_count += span;
                target.push_prop(ShapeProp {
                    name_id,
                    flags,
                    index,
                });
                (index, None)
            }
        };
        let slot_count = target.slot_count;
        let target_id = self.shapes.len() as u32;
        self.shapes.push(target);
        self.shapes[shape_id as usize]
            .transitions
            .insert((name_id, flags), target_id);
        ShapeTransition {
            shape_id: target_id,
            index,
            slot_count,
            abandoned,
        }
    }

    fn transition_in_dictionary(
        &mut self,
        shape_id: u32,
        name_id: u32,
        flags: u32,
    ) -> ShapeTransition {
        let shape = &mut self.shapes[shape_id as usize];
        let span = slot_span_for(flags);
        let existing = shape.find(name_id).map(|slot| (slot, shape.props[slot]));
        let (index, abandoned) = match existing {
            Some((slot, old)) if old.slot_span() == span => {
                shape.props[slot].flags = flags;
                (old.index, None)
            }
            Some((slot, old)) => {
                let index = shape.slot_count;
                shape.slot_count += span;
                shape.props[slot] = ShapeProp {
                    name_id,
                    flags,
                    index,
                };
                (index, Some((old.index, old.slot_span())))
            }
            None => {
                let index = shape.slot_count;
                shape.slot_count += span;
                shape.push_prop(ShapeProp {
                    name_id,
                    flags,
                    index,
                });
                (index, None)
            }
        };
        ShapeTransition {
            shape_id,
            index,
            slot_count: shape.slot_count,
            abandoned,
        }
    }

    /// 复制出一个独占的字典 shape；已是字典则原样返回。
    fn to_dictionary(&mut self, shape_id: u32) -> u32 {
        match self.shapes.get(shape_id as usize) {
            Some(shape) if shape.dictionary => shape_id,
            Some(shape) => {
                let mut dictionary = shape.clone();
                dictionary.transitions.clear();
                dictionary.dictionary = true;
                // 字典 shape 属性可能被删到很少，但查找表建了就留着——删除只会变快。
                if dictionary.index_of.is_none() {
                    dictionary.rebuild_index();
                }
                let id = self.shapes.len() as u32;
                self.shapes.push(dictionary);
                id
            }
            None => SHAPE_ID_EMPTY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: u32 = 0b111;
    const ACCESSOR: u32 = 0b111 | FLAG_IS_ACCESSOR as u32;

    #[test]
    fn identical_literals_share_one_shape() {
        let table = ShapeTable::new();
        let a = table.transition_add(ShapeTable::empty_shape(), 1, DATA);
        let ab = table.transition_add(a.shape_id, 2, DATA);
        let a2 = table.transition_add(ShapeTable::empty_shape(), 1, DATA);
        let ab2 = table.transition_add(a2.shape_id, 2, DATA);
        assert_eq!(ab.shape_id, ab2.shape_id);
        assert_eq!((a.index, ab.index), (0, 1));
        assert_eq!(table.slot_count(ab.shape_id), 2);
    }

    #[test]
    fn different_insertion_order_yields_different_shapes() {
        let table = ShapeTable::new();
        let ab = {
            let a = table.transition_add(ShapeTable::empty_shape(), 1, DATA);
            table.transition_add(a.shape_id, 2, DATA)
        };
        let ba = {
            let b = table.transition_add(ShapeTable::empty_shape(), 2, DATA);
            table.transition_add(b.shape_id, 1, DATA)
        };
        assert_ne!(ab.shape_id, ba.shape_id);
        assert_eq!(table.lookup(ba.shape_id, 1).unwrap().index, 1);
    }

    #[test]
    fn accessor_occupies_two_adjacent_slots() {
        let table = ShapeTable::new();
        let get = table.transition_add(ShapeTable::empty_shape(), 7, ACCESSOR);
        assert_eq!(get.slot_count, 2);
        let prop = table.lookup(get.shape_id, 7).unwrap();
        assert!(prop.is_accessor());
        assert_eq!((prop.getter_index(), prop.setter_index()), (0, 1));
    }

    #[test]
    fn data_to_accessor_conversion_abandons_old_slot() {
        let table = ShapeTable::new();
        let data = table.transition_add(ShapeTable::empty_shape(), 7, DATA);
        let accessor = table.transition_add(data.shape_id, 7, ACCESSOR);
        assert_eq!(accessor.abandoned, Some((0, 1)));
        assert_eq!(accessor.index, 1);
        assert_eq!(accessor.slot_count, 3);
    }

    #[test]
    fn flag_tightening_keeps_slot_and_changes_shape() {
        let table = ShapeTable::new();
        let writable = table.transition_add(ShapeTable::empty_shape(), 7, DATA);
        let frozen = table.update_flags(writable.shape_id, 7, 0).unwrap();
        assert_ne!(frozen.shape_id, writable.shape_id);
        assert_eq!(frozen.index, writable.index);
        assert_eq!(frozen.abandoned, None);
        assert_eq!(table.update_flags(writable.shape_id, 9, 0), None);
    }

    #[test]
    fn delete_degrades_to_private_dictionary() {
        let table = ShapeTable::new();
        let a = table.transition_add(ShapeTable::empty_shape(), 1, DATA);
        let ab = table.transition_add(a.shape_id, 2, DATA);
        let (dict, freed) = table.remove_prop(ab.shape_id, 1).unwrap();
        assert!(table.is_dictionary(dict));
        assert!(!table.is_dictionary(ab.shape_id));
        assert_eq!(freed, (0, 1));
        // 其余属性下标必须稳定，否则已发射的 IC 会读错槽。
        assert_eq!(table.lookup(dict, 2).unwrap().index, 1);
        assert_eq!(table.lookup(dict, 1), None);
        // 字典 shape 原地增长，不再产生新 shape。
        let grow = table.transition_add(dict, 3, DATA);
        assert_eq!(grow.shape_id, dict);
    }

    /// 空 shape 是 transition 树的根：全程序每个「作为首个属性出现的名字」都在
    /// 它上面加一条出边。高扇出必须保持共享 shape——按出边设限会让根 shape 触顶，
    /// 此后每个新对象都退化字典、inline cache 永久失效（实测 10ns → 74ns）。
    #[test]
    fn high_fanout_on_root_shape_stays_shared() {
        let table = ShapeTable::new();
        // 模拟 bootstrap：数百个不同「首属性名」的对象。
        for name_id in 1..512 {
            let step = table.transition_add(ShapeTable::empty_shape(), name_id, DATA);
            assert!(
                !table.is_dictionary(step.shape_id),
                "根 shape 的第 {name_id} 条出边不应触发字典退化"
            );
        }
        // 同结构对象仍复用同一 shape，IC 可命中。
        let a = table.transition_add(ShapeTable::empty_shape(), 7, DATA);
        let b = table.transition_add(ShapeTable::empty_shape(), 7, DATA);
        assert_eq!(a.shape_id, b.shape_id);
        assert!(!table.is_dictionary(a.shape_id));
    }

    #[test]
    fn many_props_degrade_to_dictionary() {
        let table = ShapeTable::new();
        let mut current = ShapeTable::empty_shape();
        for name_id in 0..DICTIONARY_THRESHOLD + 4 {
            current = table.transition_add(current, name_id + 1, DATA).shape_id;
        }
        assert!(table.is_dictionary(current));
        assert_eq!(table.prop_count(current), DICTIONARY_THRESHOLD + 4);
        // 退化后属性仍然全部可查，且下标保持稠密。
        for name_id in 0..DICTIONARY_THRESHOLD + 4 {
            assert_eq!(table.lookup(current, name_id + 1).unwrap().index, name_id);
        }
    }

    #[test]
    fn lookup_survives_map_threshold_crossing() {
        let table = ShapeTable::new();
        let mut current = ShapeTable::empty_shape();
        for name_id in 0..SHAPE_MAP_THRESHOLD + 3 {
            current = table.transition_add(current, name_id + 1, DATA).shape_id;
        }
        for name_id in 0..SHAPE_MAP_THRESHOLD + 3 {
            assert_eq!(table.lookup(current, name_id + 1).unwrap().index, name_id);
        }
        assert_eq!(table.lookup(current, 9999), None);
    }

    #[test]
    fn proto_generation_bumps_only_for_registered_prototypes() {
        let table = ShapeTable::new();
        assert_eq!(table.proto_generation(), 0);
        table.invalidate_if_prototype(42);
        assert_eq!(table.proto_generation(), 0);
        table.note_prototype(42);
        table.invalidate_if_prototype(42);
        assert_eq!(table.proto_generation(), 1);
        table.note_prototype(PROTO_NULL_SENTINEL);
        table.invalidate_if_prototype(PROTO_NULL_SENTINEL);
        assert_eq!(table.proto_generation(), 1);
    }

    #[test]
    fn export_import_round_trip_preserves_sharing() {
        let table = ShapeTable::new();
        let a = table.transition_add(ShapeTable::empty_shape(), 1, DATA);
        let ab = table.transition_add(a.shape_id, 2, ACCESSOR);
        table.note_prototype(5);
        table.invalidate_if_prototype(5);

        let restored = ShapeTable::new();
        restored.import(table.export());
        assert_eq!(restored.shape_count(), table.shape_count());
        assert_eq!(restored.proto_generation(), 1);
        assert_eq!(
            restored.lookup(ab.shape_id, 2),
            table.lookup(ab.shape_id, 2)
        );
        // transition 边被保留 → 恢复后同结构对象仍复用同一 shape，不产生新 id。
        let again = restored.transition_add(a.shape_id, 2, ACCESSOR);
        assert_eq!(again.shape_id, ab.shape_id);
        assert_eq!(restored.shape_count(), table.shape_count());
        // 原型登记也跟着恢复。
        restored.invalidate_if_prototype(5);
        assert_eq!(restored.proto_generation(), 2);
    }

    #[test]
    fn remap_prototypes_rewrites_registered_handles() {
        let table = ShapeTable::new();
        table.note_prototype(3);
        table.remap_prototypes(|h| h + 100);
        table.invalidate_if_prototype(3);
        assert_eq!(table.proto_generation(), 0);
        table.invalidate_if_prototype(103);
        assert_eq!(table.proto_generation(), 1);
    }
}
