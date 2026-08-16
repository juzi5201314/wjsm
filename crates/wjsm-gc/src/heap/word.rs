use std::error::Error;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct HeapAddress(u64);

impl HeapAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapMemoryError {
    AddressTooLarge {
        address: u64,
    },
    OutOfBounds {
        address: u64,
        length: u64,
        memory_len: u64,
    },
    OverlappingCopy {
        source: u64,
        destination: u64,
        length: u64,
    },
    UnalignedWord {
        address: u64,
    },
    InvalidAtomicCopyLength {
        length: u64,
    },
}

impl fmt::Display for HeapMemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressTooLarge { address } => {
                write!(formatter, "heap address {address:#x} cannot fit host usize")
            }
            Self::OutOfBounds {
                address,
                length,
                memory_len,
            } => write!(
                formatter,
                "heap range address={address:#x} length={length} exceeds memory length={memory_len}"
            ),
            Self::OverlappingCopy {
                source,
                destination,
                length,
            } => write!(
                formatter,
                "heap copy source={source:#x} destination={destination:#x} length={length} overlaps"
            ),
            Self::UnalignedWord { address } => {
                write!(
                    formatter,
                    "heap word address {address:#x} is not 8-byte aligned"
                )
            }
            Self::InvalidAtomicCopyLength { length } => {
                write!(
                    formatter,
                    "atomic heap copy length {length} is not word-aligned"
                )
            }
        }
    }
}

impl Error for HeapMemoryError {}
