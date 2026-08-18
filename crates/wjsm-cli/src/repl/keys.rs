//! ANSI escape 与 Ctrl 组合键解析。残缺序列留在缓冲里等待后续字节。

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Ctrl(char),
}

#[derive(Default)]
pub(super) struct KeyParser {
    buf: Vec<u8>,
}

impl KeyParser {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn feed(&mut self, byte: u8) {
        self.buf.push(byte);
    }

    pub(super) fn next_key(&mut self) -> Option<Key> {
        if self.buf.is_empty() {
            return None;
        }
        match self.buf[0] {
            b'\r' | b'\n' => self.take_one(Key::Enter),
            0x08 | 0x7f => self.take_one(Key::Backspace),
            0x1b => self.take_escape(),
            b if b < 0x20 => self.take_ctrl(b),
            _ => self.take_utf8(),
        }
    }

    fn take_one(&mut self, key: Key) -> Option<Key> {
        self.buf.remove(0);
        Some(key)
    }

    fn take_ctrl(&mut self, byte: u8) -> Option<Key> {
        self.buf.remove(0);
        let letter = char::from(byte + b'a' - 1);
        Some(Key::Ctrl(letter))
    }

    fn take_utf8(&mut self) -> Option<Key> {
        let Some(needed) = utf8_len(self.buf[0]) else {
            self.buf.remove(0);
            return self.next_key();
        };
        if self.buf.len() < needed {
            return None;
        }
        let bytes: Vec<u8> = self.buf.drain(..needed).collect();
        match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.chars().next())
        {
            Some(ch) => Some(Key::Char(ch)),
            None => self.next_key(),
        }
    }

    fn take_escape(&mut self) -> Option<Key> {
        if self.buf.len() < 2 {
            return None;
        }
        match self.buf[1] {
            b'[' => self.take_csi(),
            b'O' => self.take_ss3(),
            _ => {
                self.buf.remove(0);
                self.next_key()
            }
        }
    }

    fn take_ss3(&mut self) -> Option<Key> {
        if self.buf.len() < 3 {
            return None;
        }
        let ident = self.buf[2];
        self.buf.drain(..3);
        ss3_key(ident)
    }

    fn take_csi(&mut self) -> Option<Key> {
        let final_at = self
            .buf
            .iter()
            .enumerate()
            .skip(2)
            .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
            .map(|(index, _)| index)?;
        let seq: Vec<u8> = self.buf.drain(..=final_at).collect();
        csi_key(&seq[2..])
    }
}

fn utf8_len(first: u8) -> Option<usize> {
    if first < 0x80 {
        Some(1)
    } else if first & 0xe0 == 0xc0 {
        Some(2)
    } else if first & 0xf0 == 0xe0 {
        Some(3)
    } else if first & 0xf8 == 0xf0 {
        Some(4)
    } else {
        None
    }
}

fn ss3_key(ident: u8) -> Option<Key> {
    match ident {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        _ => None,
    }
}

fn csi_key(body: &[u8]) -> Option<Key> {
    let (final_byte, params) = body.split_last()?;
    match *final_byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        b'~' => csi_tilde(params),
        _ => None,
    }
}

fn csi_tilde(params: &[u8]) -> Option<Key> {
    match params {
        b"1" | b"7" => Some(Key::Home),
        b"3" => Some(Key::Delete),
        b"4" | b"8" => Some(Key::End),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyParser};

    fn parse(bytes: &[u8]) -> Vec<Key> {
        let mut parser = KeyParser::new();
        let mut keys = Vec::new();
        for &byte in bytes {
            parser.feed(byte);
            while let Some(key) = parser.next_key() {
                keys.push(key);
            }
        }
        keys
    }

    #[test]
    fn arrows_home_end_delete_and_ctrl() {
        assert_eq!(
            parse(b"\x1b[A\x1b[B\x1b[C\x1b[D"),
            [Key::Up, Key::Down, Key::Right, Key::Left]
        );
        assert_eq!(
            parse(b"\x1b[H\x1b[F\x1b[3~"),
            [Key::Home, Key::End, Key::Delete]
        );
        assert_eq!(
            parse(b"\x1b[1~\x1b[4~\x1b[7~\x1b[8~"),
            [Key::Home, Key::End, Key::Home, Key::End]
        );
        assert_eq!(parse(b"\x1bOH\x1bOF"), [Key::Home, Key::End]);
        assert_eq!(
            parse(b"\x01\x05\x03\x04"),
            [
                Key::Ctrl('a'),
                Key::Ctrl('e'),
                Key::Ctrl('c'),
                Key::Ctrl('d')
            ]
        );
        assert_eq!(
            parse(b"\r\n\x7f\x08"),
            [Key::Enter, Key::Enter, Key::Backspace, Key::Backspace]
        );
    }

    #[test]
    fn incomplete_csi_waits_then_completes() {
        let mut parser = KeyParser::new();
        parser.feed(0x1b);
        assert_eq!(parser.next_key(), None);
        parser.feed(b'[');
        assert_eq!(parser.next_key(), None);
        parser.feed(b'A');
        assert_eq!(parser.next_key(), Some(Key::Up));
        assert_eq!(parser.next_key(), None);
    }

    #[test]
    fn utf8_cjk_waits_for_full_sequence() {
        let mut parser = KeyParser::new();
        let zhong = "中".as_bytes();
        parser.feed(zhong[0]);
        assert_eq!(parser.next_key(), None);
        parser.feed(zhong[1]);
        assert_eq!(parser.next_key(), None);
        parser.feed(zhong[2]);
        assert_eq!(parser.next_key(), Some(Key::Char('中')));
    }
}
