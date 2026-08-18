//! 读一行：按键 → 行缓冲 / 历史 → 重绘光标列。

use std::io::{self, Read, Write};

use super::PROMPT;
use super::buffer::LineBuffer;
use super::history::History;
use super::keys::{Key, KeyParser};
use super::width::display_width;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ReadOutcome {
    Line(String),
    Eof,
}

enum Action {
    Continue,
    Submit,
    Eof,
}

pub(super) struct Editor {
    buffer: LineBuffer,
    history: History,
    parser: KeyParser,
}

impl Editor {
    pub(super) fn new() -> Self {
        Self {
            buffer: LineBuffer::new(),
            history: History::new(),
            parser: KeyParser::new(),
        }
    }

    pub(super) fn read_line<R: Read, W: Write>(
        &mut self,
        input: &mut R,
        output: &mut W,
    ) -> io::Result<ReadOutcome> {
        self.buffer.clear();
        self.history.reset_browse();
        redraw(output, &self.buffer)?;
        let mut chunk = [0u8; 64];
        loop {
            let count = input.read(&mut chunk)?;
            if count == 0 {
                return Ok(ReadOutcome::Eof);
            }
            for &byte in &chunk[..count] {
                self.parser.feed(byte);
            }
            while let Some(key) = self.parser.next_key() {
                match self.apply_key(key) {
                    Action::Continue => redraw(output, &self.buffer)?,
                    Action::Submit => {
                        writeln!(output)?;
                        return Ok(ReadOutcome::Line(self.take_line()));
                    }
                    Action::Eof => return Ok(ReadOutcome::Eof),
                }
            }
        }
    }

    fn apply_key(&mut self, key: Key) -> Action {
        match key {
            Key::Char(ch) if !ch.is_control() => {
                self.buffer.insert(ch);
                Action::Continue
            }
            Key::Char(_) => Action::Continue,
            Key::Enter => Action::Submit,
            Key::Backspace => {
                self.buffer.backspace();
                Action::Continue
            }
            Key::Delete => {
                self.buffer.delete();
                Action::Continue
            }
            Key::Left => {
                self.buffer.move_left();
                Action::Continue
            }
            Key::Right => {
                self.buffer.move_right();
                Action::Continue
            }
            Key::Home | Key::Ctrl('a') => {
                self.buffer.move_home();
                Action::Continue
            }
            Key::End | Key::Ctrl('e') => {
                self.buffer.move_end();
                Action::Continue
            }
            Key::Up => self.recall_up(),
            Key::Down => self.recall_down(),
            Key::Ctrl('c') => {
                self.buffer.clear();
                self.history.reset_browse();
                Action::Continue
            }
            Key::Ctrl('d') if self.buffer.as_str().is_empty() => Action::Eof,
            Key::Ctrl('d') => {
                self.buffer.delete();
                Action::Continue
            }
            Key::Ctrl(_) => Action::Continue,
        }
    }

    fn recall_up(&mut self) -> Action {
        if let Some(line) = self.history.up(self.buffer.as_str()) {
            self.buffer.replace(line);
        }
        Action::Continue
    }

    fn recall_down(&mut self) -> Action {
        if let Some(line) = self.history.down() {
            self.buffer.replace(line);
        }
        Action::Continue
    }

    fn take_line(&mut self) -> String {
        let line = self.buffer.take();
        if !line.trim().is_empty() {
            self.history.push(line.trim().to_string());
        }
        line
    }
}

fn redraw<W: Write>(output: &mut W, buffer: &LineBuffer) -> io::Result<()> {
    write!(output, "\r{PROMPT}{}\x1b[K", buffer.as_str())?;
    let column = display_width(PROMPT) + buffer.display_column();
    if column == 0 {
        write!(output, "\r")?;
    } else {
        write!(output, "\r\x1b[{column}C")?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::{Editor, ReadOutcome};

    fn read(bytes: &[u8]) -> (ReadOutcome, String) {
        let mut editor = Editor::new();
        let mut output = Vec::new();
        let outcome = editor
            .read_line(&mut &bytes[..], &mut output)
            .expect("editor");
        (outcome, String::from_utf8_lossy(&output).into_owned())
    }

    #[test]
    fn inserts_cjk_then_left_and_ascii() {
        let mut input = Vec::new();
        input.extend_from_slice("你".as_bytes());
        input.extend_from_slice(b"\x1b[D");
        input.push(b'x');
        input.push(b'\r');
        let (outcome, _) = read(&input);
        assert_eq!(outcome, ReadOutcome::Line("x你".into()));
    }

    #[test]
    fn history_up_recalls_previous_line() {
        let mut editor = Editor::new();
        let mut output = Vec::new();
        let first = editor
            .read_line(&mut &b"1+1\r"[..], &mut output)
            .expect("first");
        assert_eq!(first, ReadOutcome::Line("1+1".into()));
        let second = editor
            .read_line(&mut &b"\x1b[A\r"[..], &mut output)
            .expect("second");
        assert_eq!(second, ReadOutcome::Line("1+1".into()));
    }

    #[test]
    fn empty_ctrl_d_is_eof() {
        let (outcome, _) = read(b"\x04");
        assert_eq!(outcome, ReadOutcome::Eof);
    }

    #[test]
    fn redraw_places_cursor_after_wide_char() {
        let mut input = Vec::new();
        input.extend_from_slice("中".as_bytes());
        // 不再提交，补一个未完成的 ESC 让 read 在下一轮读到 0 之前先重绘。
        // 用 Enter 收尾并检查提交文本即可；列宽由 buffer 单测覆盖。
        input.push(b'\r');
        let (outcome, drawn) = read(&input);
        assert_eq!(outcome, ReadOutcome::Line("中".into()));
        assert!(drawn.contains("中"), "{drawn:?}");
        assert!(drawn.contains("\x1b[8C"), "{drawn:?}");
    }
}
