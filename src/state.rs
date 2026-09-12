//! Буфер набранных нажатий.
//!
//! Хранятся именно скан-коды, а не символы: чтобы исправить раскладку,
//! достаточно переиграть те же самые физические клавиши после её переключения.

use crate::keys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub shift: bool,
}

pub struct Buffer {
    strokes: Vec<Stroke>,
    max: usize,
}

impl Buffer {
    pub fn new(max: usize) -> Self {
        Buffer {
            strokes: Vec::new(),
            max: max.max(1),
        }
    }

    pub fn push(&mut self, code: u16, shift: bool) {
        self.strokes.push(Stroke { code, shift });
        if self.strokes.len() > self.max {
            let overflow = self.strokes.len() - self.max;
            self.strokes.drain(..overflow);
        }
    }

    pub fn backspace(&mut self) {
        self.strokes.pop();
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
    }

    /// Вся фраза целиком.
    pub fn phrase(&self) -> &[Stroke] {
        &self.strokes
    }

    /// Последнее слово вместе с хвостовыми разделителями.
    ///
    /// Хвост включается намеренно: курсор стоит уже после пробела, поэтому
    /// стирать и набирать заново надо вместе с ним, иначе слово склеится.
    pub fn last_word(&self) -> &[Stroke] {
        let mut end = self.strokes.len();
        while end > 0 && keys::is_separator(self.strokes[end - 1].code) {
            end -= 1;
        }
        if end == 0 {
            return &[];
        }
        let mut start = end;
        while start > 0 && !keys::is_separator(self.strokes[start - 1].code) {
            start -= 1;
        }
        &self.strokes[start..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KEY_SPACE;

    /// `q w` — буквы q(16), w(17).
    fn filled() -> Buffer {
        let mut b = Buffer::new(64);
        b.push(16, false);
        b.push(KEY_SPACE, false);
        b.push(17, true);
        b
    }

    #[test]
    fn last_word_takes_only_trailing_word() {
        let b = filled();
        assert_eq!(
            b.last_word(),
            &[Stroke {
                code: 17,
                shift: true
            }]
        );
    }

    #[test]
    fn last_word_keeps_trailing_separators() {
        let mut b = filled();
        b.push(KEY_SPACE, false);
        let word = b.last_word();
        assert_eq!(word.len(), 2, "слово + хвостовой пробел");
        assert_eq!(word[0].code, 17);
        assert_eq!(word[1].code, KEY_SPACE);
    }

    #[test]
    fn last_word_empty_when_only_separators() {
        let mut b = Buffer::new(64);
        b.push(KEY_SPACE, false);
        b.push(KEY_SPACE, false);
        assert!(b.last_word().is_empty());
    }

    #[test]
    fn phrase_returns_everything() {
        assert_eq!(filled().phrase().len(), 3);
    }

    #[test]
    fn backspace_pops_last_stroke() {
        let mut b = filled();
        b.backspace();
        assert_eq!(b.phrase().len(), 2);
    }

    #[test]
    fn buffer_is_capped_and_drops_oldest() {
        let mut b = Buffer::new(2);
        b.push(16, false);
        b.push(17, false);
        b.push(18, false);
        assert_eq!(b.phrase().len(), 2);
        assert_eq!(b.phrase()[0].code, 17, "самое старое нажатие вытеснено");
    }
}
