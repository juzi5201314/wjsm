//! GC 算法注册表。
//!
//! 本模块只负责把装配层选择解析为具体算法实例；尚未落地的算法在这里显式拒绝，
//! 不提供行为 stub。

use std::str::FromStr;

use crate::runtime_gc::api::GcAlgorithm;
use crate::runtime_gc::mark_sweep::MarkSweepCollector;

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

/// 按种类创建 GC 算法实例。
pub fn create(kind: GcAlgorithmKind) -> Result<Box<dyn GcAlgorithm + Send + Sync>, String> {
    match kind {
        GcAlgorithmKind::MarkSweep => Ok(Box::new(MarkSweepCollector::new())),
        GcAlgorithmKind::G1 | GcAlgorithmKind::Zgc => Err(format!(
            "GC algorithm `{}` is registered but not implemented yet; currently available: mark-sweep",
            kind.as_str()
        )),
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

    #[test]
    fn create_mark_sweep_succeeds() {
        assert!(create(GcAlgorithmKind::MarkSweep).is_ok());
    }

    #[test]
    fn create_rejects_future_algorithms_clearly() {
        for kind in [GcAlgorithmKind::G1, GcAlgorithmKind::Zgc] {
            let err = match create(kind) {
                Ok(_) => panic!("{} should be rejected before implementation", kind.as_str()),
                Err(err) => err,
            };

            assert!(err.contains(kind.as_str()));
            assert!(err.contains("not implemented"));
            assert!(err.contains("mark-sweep"));
        }
    }
}
