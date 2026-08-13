//! 数组排序核心：可失败的稳定自然归并排序。
//!
//! ECMAScript 的 `Array.prototype.sort` 要求稳定排序，而 comparator 是任意
//! JavaScript 函数：可能返回不一致、非全序的结果，也可能抛出异常。标准库
//! `slice::sort_by` 要求 comparator 实现全序，无法直接承载任意 JS comparator，
//! 因此这里提供私有算法入口：稳定、可失败、自适应，最坏情况 O(n log n) 次比较。

use std::ops::Range;

/// 稳定自然归并排序。
///
/// 算法分两阶段循环：
/// 1. 单次线性扫描识别自然 run：相邻比较为 `Greater` 时扩展严格降序 run，只反转
///    严格降序尾部并把其前非降序前缀压为独立 run（反转后前缀与尾部的边界不一定
///    成立），遇到 `Equal` 即结束降序 run，避免反转相等元素破坏稳定性；其余扩展
///    非降序 run。若整个输入已是单个 run，直接返回。
/// 2. 每轮按相邻 run 成对归并，比较相等时总是选择左 run；每对归并写入 scratch
///    对应区间，未配对的尾 run 用 `copy_from_slice` 从当前 `values` 复制到
///    scratch。整轮全部成功后才把 scratch 完整复制回 `values`，下一轮使用合并
///    后的连续区间，直至只剩单个 run。
///
/// 该结构对 r 个自然 run 执行 O(n log r) 次数据移动、O(n log r) 次比较
/// （最坏 O(n log n)）；已排序与严格逆序输入只需线性 run 检测。每次比较都用
/// `?` 立即传播 `E`，错误发生后不再调用 comparator。helper 不承诺出错时保持
/// 其私有 slice 内容不变，调用方必须在成功前只操作暂存数据，从而保证异常时
/// 不写回用户数组。
pub(super) fn stable_sort_by<T: Copy, E>(
    values: &mut [T],
    mut compare: impl FnMut(T, T) -> Result<std::cmp::Ordering, E>,
) -> Result<(), E> {
    let n = values.len();
    if n <= 1 {
        return Ok(());
    }
    let mut runs = collect_runs(values, &mut compare)?;
    if runs.len() <= 1 {
        return Ok(());
    }
    let mut scratch = values.to_vec();
    loop {
        let mut next = Vec::with_capacity(runs.len().div_ceil(2));
        let mut write = 0;
        for pair in runs.chunks_exact(2) {
            let (left, right) = (pair[0].clone(), pair[1].clone());
            merge_into(
                values,
                &left,
                &right,
                &mut scratch[write..write + left.len() + right.len()],
                &mut compare,
            )?;
            write += left.len() + right.len();
            next.push(write - left.len() - right.len()..write);
        }
        if let Some(last) = runs.last() {
            if runs.len() % 2 == 1 {
                scratch[write..write + last.len()].copy_from_slice(&values[last.clone()]);
                next.push(write..write + last.len());
            }
        }
        values.copy_from_slice(&scratch);
        runs = next;
        if runs.len() <= 1 {
            return Ok(());
        }
    }
}

/// 单次线性扫描识别自然 run。
///
/// 相邻比较为 `Greater` 时扩展严格降序 run；遇到 `Equal` 结束降序 run（否则
/// 反转会破坏相等元素的稳定性）。降序尾部 `end-1..descending_end` 反转后与
/// 前方非降序前缀 `start..end-1` 的边界不一定成立，因此前缀作为独立 run 压入。
/// 返回的区间为左闭右开且互不重叠、按序覆盖整个 `values`。
fn collect_runs<T: Copy, E>(
    values: &mut [T],
    compare: &mut impl FnMut(T, T) -> Result<std::cmp::Ordering, E>,
) -> Result<Vec<Range<usize>>, E> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut end = 1;
    while end < values.len() {
        if compare(values[end - 1], values[end])? == std::cmp::Ordering::Greater {
            let mut descending_end = end + 1;
            while descending_end < values.len()
                && compare(values[descending_end - 1], values[descending_end])?
                    == std::cmp::Ordering::Greater
            {
                descending_end += 1;
            }
            // 降序尾 `end-1..descending_end` 反转后与前方非降序前缀
            // `start..end-1` 的边界不一定成立，因此前缀必须作为独立 run 压入；
            // 只反转严格降序尾部，避免反转相等元素破坏稳定性。
            if start < end - 1 {
                runs.push(start..end - 1);
            }
            values[end - 1..descending_end].reverse();
            runs.push(end - 1..descending_end);
            start = descending_end;
            end = descending_end + 1;
        } else {
            end += 1;
        }
    }
    if start < values.len() {
        runs.push(start..values.len());
    }
    Ok(runs)
}

/// 归并两个相邻 run 到 `out`（长度恰为两 run 长度之和）。
///
/// 相等时选择左 run 元素，保证稳定性；比较失败立即返回，不写回 `values`。
fn merge_into<T: Copy, E>(
    values: &[T],
    left: &Range<usize>,
    right: &Range<usize>,
    out: &mut [T],
    compare: &mut impl FnMut(T, T) -> Result<std::cmp::Ordering, E>,
) -> Result<(), E> {
    let (mut i, mut j) = (left.start, right.start);
    let mut write = 0;
    while i < left.end && j < right.end {
        if compare(values[i], values[j])? == std::cmp::Ordering::Greater {
            out[write] = values[j];
            j += 1;
        } else {
            out[write] = values[i];
            i += 1;
        }
        write += 1;
    }
    while i < left.end {
        out[write] = values[i];
        i += 1;
        write += 1;
    }
    while j < right.end {
        out[write] = values[j];
        j += 1;
        write += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::stable_sort_by;
    use std::cmp::Ordering;

    /// 哨兵错误：第 `at` 次比较时返回。
    #[derive(Debug, PartialEq)]
    struct SentryError;

    /// 带计数的可失败比较器；`fail_at` 次比较后返回哨兵错误。
    struct CountingCompare {
        calls: usize,
        fail_at: Option<usize>,
    }

    impl CountingCompare {
        fn new(fail_at: Option<usize>) -> Self {
            Self { calls: 0, fail_at }
        }

        fn call(&mut self, left: u32, right: u32) -> Result<Ordering, SentryError> {
            self.calls += 1;
            if self.fail_at == Some(self.calls) {
                return Err(SentryError);
            }
            Ok(left.cmp(&right))
        }
    }

    #[test]
    fn stable_for_duplicate_keys() {
        let mut input = [(1, "a"), (0, "b"), (1, "c"), (0, "d")];
        stable_sort_by(&mut input, |(ka, _), (kb, _)| -> Result<_, ()> {
            Ok(ka.cmp(&kb))
        })
        .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
        assert_eq!(input, [(0, "b"), (0, "d"), (1, "a"), (1, "c")]);
    }

    #[test]
    fn prng_input_uses_logarithmic_comparisons() {
        let mut input: Vec<(u32, u32)> = (0..1000).map(|i| ((i * 7919) % 10007, i)).collect();
        let mut calls = 0;
        stable_sort_by(&mut input, |(a, _), (b, _)| -> Result<_, ()> {
            calls += 1;
            Ok(a.cmp(&b))
        })
        .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
        assert!(input.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(input.windows(2).all(|w| w[0].0 < w[1].0 || w[0].1 < w[1].1));
        assert!(calls <= 12_000, "comparator calls: {calls}");
    }

    #[test]
    fn sorted_and_reverse_sorted_inputs_are_linear() {
        for (input, expected_first) in [
            ((0..1000).collect::<Vec<_>>(), 0),
            ((0..1000).rev().collect::<Vec<_>>(), 0),
        ] {
            let mut calls = 0;
            let mut values = input.clone();
            stable_sort_by(&mut values, |a, b| -> Result<_, ()> {
                calls += 1;
                Ok(a.cmp(&b))
            })
            .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
            assert_eq!(values.first(), Some(&expected_first));
            assert!(values.windows(2).all(|w| w[0] <= w[1]));
            assert!(calls <= 1_000, "comparator calls: {calls}");
        }
    }

    #[test]
    fn propagates_error_and_stops_comparing() {
        let mut input: Vec<u32> = (0..1000).rev().collect();
        let mut compare = CountingCompare::new(Some(3));
        let error = stable_sort_by(&mut input, |a, b| compare.call(a, b)).unwrap_err();
        assert_eq!(error, SentryError);
        assert_eq!(compare.calls, 3);
    }

    #[test]
    fn arbitrary_comparators_never_panic() {
        let input: Vec<u32> = (0..1000).collect();
        let cases: [(&str, fn(u32, u32) -> Ordering, Vec<u32>); 3] = [
            ("all equal", |_, _| Ordering::Equal, input.clone()),
            ("reverse", |a, b| b.cmp(&a), (0..1000).rev().collect()),
            ("identity", |a, b| a.cmp(&b), input.clone()),
        ];
        for (name, comparator, expected) in cases {
            let mut values = input.clone();
            stable_sort_by(&mut values, |a, b| -> Result<_, ()> {
                Ok(comparator(a, b))
            })
            .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
            assert_eq!(values, expected, "comparator: {name}");
        }
    }

    #[test]
    fn single_and_empty_inputs_never_compare() {
        let mut calls = 0;
        let mut single = [42u32];
        stable_sort_by(&mut single, |a, b| -> Result<_, ()> {
            calls += 1;
            Ok(a.cmp(&b))
        })
        .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
        let mut empty: [u32; 0] = [];
        stable_sort_by(&mut empty, |a, b| -> Result<_, ()> {
            calls += 1;
            Ok(a.cmp(&b))
        })
        .unwrap_or_else(|_| unreachable!("comparator cannot fail"));
        assert_eq!(calls, 0);
    }
}
