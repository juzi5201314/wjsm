//! 字符串内容哈希的权威实现（固定种子）。
//!
//! 常量字符串的 hash 与 Latin-1/UTF-16 表示在编译期烘焙进 `.wjsm`，运行时
//! （制品解码、install 期发布、堆内惰性填充）一律复用本模块同一函数，跨进程、
//! 跨表示严格同值——这是「同内容属性名 → 同句柄」去重不变式的硬前提：常量
//! `"foo"` 与运行时拼出的 `"foo"` 若哈希不同，内容去重表会把同名属性键分裂成
//! 两个句柄。因此种子必须是编译期常数；随机种子会让烘焙值随进程漂移。
//! 代价是放弃进程级随机化带来的防构造碰撞特性（murmur 风格混合仍在，意外
//! 碰撞概率不变，只有蓄意碰撞变得可构造）；该哈希仅用作内容去重键，本仓库
//! 的威胁模型接受这一取舍。

/// 固定哈希种子。已纳入 `heap_layout_abi_inputs`，变更即快照 ABI 失效。
pub const STRING_HASH_SEED: u32 = 0x9e37_79b9;

/// 哈希未计算哨兵：与堆内字符串头 `+24` 的 0 语义一致，真实哈希归一化到非 0。
pub const HASH_UNCOMPUTED: u32 = 0;

/// 计算 UTF-16 码元序列的内容哈希。
pub fn content_hash_units(units: &[u16]) -> u32 {
    compute_hash_by_len(units.len(), |index| units[index])
}

/// 计算 Latin-1 单字节载荷的内容哈希；字节零扩展为码元，与
/// [`content_hash_units`] 对同内容给出同值（跨表示去重的键一致性）。
pub fn content_hash_latin1(bytes: &[u8]) -> u32 {
    compute_hash_by_len(bytes.len(), |index| u16::from(bytes[index]))
}

/// UTF-16 内容哈希：每次吃 4 个码元（8 字节），双累加器交替。
///
/// 逐码元做一次乘法会形成长度成正比的串行乘法依赖链，中等长度串上比它要取代的
/// SipHash 还慢；按机器字打包并拆成两条独立链后，吞吐由乘法端口而非延迟决定。
/// `read_unit` 按码元下标读取（Latin-1 逐字节零扩展、UTF-16 逐字），闭包在
/// 调用方编译期内联。结果归一化到非 [`HASH_UNCOMPUTED`]。
pub fn compute_hash_by_len(len: usize, mut read_unit: impl FnMut(usize) -> u16) -> u32 {
    const K1: u64 = 0xff51_afd7_ed55_8ccd;
    const K2: u64 = 0xc4ce_b9fe_1a85_ec53;

    let mut left = u64::from(STRING_HASH_SEED) ^ (len as u64).wrapping_mul(K1);
    let mut right = K2;
    let full_chunks = len / 8;
    for chunk in 0..full_chunks {
        let base = chunk * 8;
        let mut low = 0_u64;
        let mut high = 0_u64;
        for j in 0..4 {
            low |= u64::from(read_unit(base + j)) << (j * 16);
        }
        for j in 0..4 {
            high |= u64::from(read_unit(base + 4 + j)) << (j * 16);
        }
        left = (left ^ low).wrapping_mul(K1).rotate_left(31);
        right = (right ^ high).wrapping_mul(K2).rotate_left(29);
    }
    let mut tail = 0_u64;
    for index in (full_chunks * 8)..len {
        tail |= u64::from(read_unit(index)) << ((index % 4) * 16);
        if index % 4 == 3 {
            left = (left ^ tail).wrapping_mul(K1).rotate_left(31);
            tail = 0;
        }
    }
    right ^= tail;

    let mut mixed = left ^ right.wrapping_mul(K1);
    mixed ^= mixed >> 33;
    mixed = mixed.wrapping_mul(K2);
    mixed ^= mixed >> 29;
    let hash = (mixed as u32) ^ ((mixed >> 32) as u32);
    if hash == HASH_UNCOMPUTED { 1 } else { hash }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_and_units_hash_equal_for_same_content() {
        let units: Vec<u16> = "wjsm".encode_utf16().collect();
        assert_eq!(content_hash_units(&units), content_hash_latin1(b"wjsm"));
    }

    #[test]
    fn hash_is_deterministic_and_length_sensitive() {
        let a: Vec<u16> = "abc".encode_utf16().collect();
        let b: Vec<u16> = "abd".encode_utf16().collect();
        let empty: Vec<u16> = Vec::new();
        assert_eq!(content_hash_units(&a), content_hash_units(&a));
        assert_ne!(content_hash_units(&a), content_hash_units(&b));
        assert_ne!(content_hash_units(&a), content_hash_units(&empty));
        assert_ne!(content_hash_units(&empty), HASH_UNCOMPUTED);
    }

    #[test]
    fn hash_covers_long_and_tailed_content() {
        // 覆盖 8 码元整块、尾块（1/4/5 码元）与多字节路径。
        for len in [0_usize, 1, 4, 5, 8, 9, 12, 13, 100] {
            let units: Vec<u16> = (0..len)
                .map(|i| u16::try_from(i % 0x2_ffff).unwrap())
                .collect();
            let hash = content_hash_units(&units);
            assert_ne!(hash, HASH_UNCOMPUTED, "len={len}");
            // 同内容 Latin-1 子集（前 256 个码元）跨表示同哈希。
            let latin_units: Vec<u16> = (0..len.min(255))
                .map(|i| u16::try_from(i).unwrap())
                .collect();
            let bytes: Vec<u8> = latin_units
                .iter()
                .map(|u| u8::try_from(*u).unwrap())
                .collect();
            assert_eq!(
                content_hash_units(&latin_units),
                content_hash_latin1(&bytes)
            );
        }
    }
}
