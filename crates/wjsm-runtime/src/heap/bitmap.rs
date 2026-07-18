use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct AtomicBitmap {
    bits: usize,
    words: Box<[AtomicU64]>,
}

impl AtomicBitmap {
    pub(crate) fn new(bits: usize) -> Self {
        let words = bits.div_ceil(u64::BITS as usize);
        let words = std::iter::repeat_with(|| AtomicU64::new(0))
            .take(words)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self { bits, words }
    }

    pub(crate) fn mark(&self, bit: usize) {
        debug_assert!(bit < self.bits);
        let word = bit / u64::BITS as usize;
        let mask = 1_u64 << (bit % u64::BITS as usize);
        self.words[word].fetch_or(mask, Ordering::Release);
    }

    pub(crate) fn clear(&self) {
        for word in &self.words {
            word.store(0, Ordering::Release);
        }
    }

    pub(crate) fn is_marked(&self, bit: usize) -> bool {
        debug_assert!(bit < self.bits);
        let word = bit / u64::BITS as usize;
        let mask = 1_u64 << (bit % u64::BITS as usize);
        self.words[word].load(Ordering::Acquire) & mask != 0
    }

    pub(crate) fn next_set_from(&self, bit: usize) -> Option<usize> {
        if bit >= self.bits {
            return None;
        }
        let mut word_index = bit / u64::BITS as usize;
        let mut word = self.words[word_index].load(Ordering::Acquire);
        word &= !0_u64 << (bit % u64::BITS as usize);
        while word == 0 {
            word_index += 1;
            if word_index == self.words.len() {
                return None;
            }
            word = self.words[word_index].load(Ordering::Acquire);
        }
        let next = word_index * u64::BITS as usize + word.trailing_zeros() as usize;
        (next < self.bits).then_some(next)
    }

    pub(crate) fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.load(Ordering::Acquire).count_ones() as usize)
            .sum()
    }
}
