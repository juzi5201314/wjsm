//! GC 算法种类注册表。
//!
//! active collect 由 `active_v2` / `active_zgc` 按 `GcAlgorithmKind` 分派；
//! 不再构造 V1 `Box<dyn GcAlgorithm>`。

use std::str::FromStr;

const VALID_ALGORITHMS: &str = "mark-sweep, g1, zgc";

/// 可由装配层选择的 GC 算法种类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GcAlgorithmKind {
    MarkSweep,
    G1,
    Zgc,
}

impl FromStr for GcAlgorithmKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mark-sweep" => Ok(Self::MarkSweep),
            "g1" => Ok(Self::G1),
            "zgc" => Ok(Self::Zgc),
            other => Err(format!(
                "unknown GC algorithm `{other}`; expected one of: {VALID_ALGORITHMS}"
            )),
        }
    }
}

impl GcAlgorithmKind {
    /// 返回该算法在配置/CLI 边界使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MarkSweep => "mark-sweep",
            Self::G1 => "g1",
            Self::Zgc => "zgc",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_accepts_known_names() {
        assert_eq!(
            "mark-sweep".parse::<GcAlgorithmKind>().unwrap(),
            GcAlgorithmKind::MarkSweep
        );
        assert_eq!(
            "g1".parse::<GcAlgorithmKind>().unwrap(),
            GcAlgorithmKind::G1
        );
        assert_eq!(
            "zgc".parse::<GcAlgorithmKind>().unwrap(),
            GcAlgorithmKind::Zgc
        );
    }

    #[test]
    fn from_str_rejects_non_exact_names_with_legal_values() {
        for input in ["marksweep", "mark_sweep", "MarkSweep", " mark-sweep"] {
            let err = input.parse::<GcAlgorithmKind>().unwrap_err();

            assert!(err.contains(input));
            assert!(err.contains("mark-sweep"));
            assert!(err.contains("g1"));
            assert!(err.contains("zgc"));
        }
    }

    #[test]
    fn as_str_returns_stable_names() {
        assert_eq!(GcAlgorithmKind::MarkSweep.as_str(), "mark-sweep");
        assert_eq!(GcAlgorithmKind::G1.as_str(), "g1");
        assert_eq!(GcAlgorithmKind::Zgc.as_str(), "zgc");
    }
}
