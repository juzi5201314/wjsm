//! grapheme 级行缓冲：插入、删除、左右移动、行首行尾。

use wjsm_intl_data::{OwnedSegmenter, SegmentGranularity};

use super::width::prefix_width;

#[derive(Default)]
pub(super) struct LineBuffer {
    text: String,
    cursor: usize,
}

impl LineBuffer {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn as_str(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn display_column(&self) -> usize {
        prefix_width(&self.text, self.cursor)
    }

    pub(super) fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub(super) fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.text.replace_range(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    pub(super) fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    pub(super) fn move_left(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.cursor = prev;
        }
    }

    pub(super) fn move_right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.cursor = next;
        }
    }

    pub(super) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub(super) fn replace(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub(super) fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn bounds(&self) -> Vec<usize> {
        let mut bounds =
            OwnedSegmenter::new(SegmentGranularity::Grapheme).break_offsets(&self.text);
        if bounds.is_empty() {
            bounds.push(0);
        }
        if *bounds.last().expect("bounds 至少含 0") != self.text.len() {
            bounds.push(self.text.len());
        }
        bounds
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.bounds()
            .into_iter()
            .rev()
            .find(|&bound| bound < self.cursor)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.bounds().into_iter().find(|&bound| bound > self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::LineBuffer;

    #[test]
    fn insert_delete_and_move_by_grapheme() {
        let mut line = LineBuffer::new();
        for ch in "你好".chars() {
            line.insert(ch);
        }
        assert_eq!(line.as_str(), "你好");
        assert_eq!(line.display_column(), 4);
        line.move_left();
        assert_eq!(line.display_column(), 2);
        line.insert('x');
        assert_eq!(line.as_str(), "你x好");
        line.backspace();
        assert_eq!(line.as_str(), "你好");
        line.move_home();
        line.delete();
        assert_eq!(line.as_str(), "好");
        line.move_end();
        line.backspace();
        assert_eq!(line.as_str(), "");
    }

    #[test]
    fn combining_and_zwj_are_single_units() {
        let mut line = LineBuffer::new();
        for ch in "e\u{0301}".chars() {
            line.insert(ch);
        }
        assert_eq!(line.as_str(), "e\u{0301}");
        assert_eq!(line.display_column(), 1);
        line.move_left();
        assert_eq!(line.cursor(), 0);
        line.move_right();
        assert_eq!(line.cursor(), line.as_str().len());

        line.clear();
        for ch in "👨‍👩‍👧".chars() {
            line.insert(ch);
        }
        assert_eq!(line.display_column(), 2);
        line.move_left();
        assert_eq!(line.cursor(), 0);
        line.delete();
        assert_eq!(line.as_str(), "");
    }
}
