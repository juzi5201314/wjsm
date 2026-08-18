//! 会话内历史：上下键浏览副本，不改已提交项，不落盘。

#[derive(Default)]
pub(super) struct History {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl History {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn push(&mut self, line: String) {
        if line.is_empty() {
            return;
        }
        if self.entries.last() != Some(&line) {
            self.entries.push(line);
        }
        self.reset_browse();
    }

    pub(super) fn up(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.cursor {
            None => {
                self.draft = current.to_string();
                let index = self.entries.len() - 1;
                self.cursor = Some(index);
                Some(self.entries[index].clone())
            }
            Some(0) => Some(self.entries[0].clone()),
            Some(index) => {
                let index = index - 1;
                self.cursor = Some(index);
                Some(self.entries[index].clone())
            }
        }
    }

    pub(super) fn down(&mut self) -> Option<String> {
        let Some(index) = self.cursor else {
            return None;
        };
        if index + 1 < self.entries.len() {
            let index = index + 1;
            self.cursor = Some(index);
            Some(self.entries[index].clone())
        } else {
            self.cursor = None;
            Some(self.draft.clone())
        }
    }

    pub(super) fn reset_browse(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::History;

    #[test]
    fn browse_copies_and_restores_draft() {
        let mut history = History::new();
        history.push("1+1".into());
        history.push("2*3".into());
        assert_eq!(history.up("draft").as_deref(), Some("2*3"));
        assert_eq!(history.up("draft").as_deref(), Some("1+1"));
        assert_eq!(history.up("draft").as_deref(), Some("1+1"));
        assert_eq!(history.down().as_deref(), Some("2*3"));
        assert_eq!(history.down().as_deref(), Some("draft"));
        assert_eq!(history.down(), None);
    }
}
