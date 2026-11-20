//! Native terminal state. Session threads parse output; the UI samples dirty frames.
use std::num::NonZeroU16;

use slint::Color;

use crate::TerminalCell;

include!(concat!(env!("OUT_DIR"), "/terminal_palettes.rs"));

#[derive(Default)]
pub struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        prefix: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        code: char,
    ) {
        let arg = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (prefix, code, arg) {
            (None, 'n', 5) => "\x1b[0n".to_owned(),
            (None | Some(b'?'), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!(
                    "\x1b[{}{};{}R",
                    if prefix.is_some() { "?" } else { "" },
                    row + 1,
                    col + 1
                )
            }
            (None, 'c', 0) => "\x1b[?1;2c".to_owned(),
            _ => return,
        };
        if self.0.len() + reply.len() <= 65536 {
            self.0.extend_from_slice(reply.as_bytes());
        }
    }
}

pub struct Terminal {
    pub parser: vt100::Parser<Replies>,
    pub dirty: bool,
    theme: usize,
    selection: Option<((u16, u16), (u16, u16))>,
    pub highlighting: bool,
    pub highlight_colors: [Color; 6],
    find_query: String,
    find_position: Option<(usize, u16)>,
}

impl Terminal {
    #[cfg(test)]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::with_scrollback(rows, cols, 10000)
    }

    pub fn with_scrollback(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nonzero(rows),
                nonzero(cols),
                scrollback.clamp(1000, 100000),
                Replies::default(),
            ),
            dirty: true,
            theme: 0,
            selection: None,
            highlighting: false,
            highlight_colors: [Color::default(); 6],
            find_query: String::new(),
            find_position: None,
        }
    }

    pub fn set_theme(&mut self, theme: i32) {
        let theme = theme.clamp(0, 3) as usize;
        if self.theme != theme {
            self.theme = theme;
            self.dirty = true;
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.selection = None;
        self.parser.process(bytes);
        self.dirty = true;
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        if self.parser.screen().size() == (nonzero(rows), nonzero(cols)) {
            return false;
        }
        self.parser.set_size(nonzero(rows), nonzero(cols));
        self.selection = None;
        self.dirty = true;
        true
    }

    pub fn scroll(&mut self, lines: i32) {
        if self.parser.screen().alternate_screen() {
            return;
        }
        let offset = self
            .parser
            .screen()
            .scrollback()
            .saturating_add_signed(lines as isize);
        self.parser.screen_mut().set_scrollback(offset);
        self.selection = None;
        self.dirty = true;
    }

    pub fn follow_output(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
        self.dirty = true;
    }

    pub fn replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }

    pub fn paste(&self, text: &str) -> Vec<u8> {
        // Control sequences in clipboard text must not escape bracketed-paste mode.
        let clean: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        if self.parser.screen().bracketed_paste() {
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            clean.replace('\n', "\r").into_bytes()
        }
    }

    pub fn select(&mut self, row: i32, col: i32, start: bool) {
        let (rows, cols) = self.parser.screen().size();
        let point = (
            row.clamp(0, i32::from(rows.get()) - 1) as u16,
            col.clamp(0, i32::from(cols.get())) as u16,
        );
        if start {
            self.selection = Some((point, point));
        } else if let Some((_, end)) = &mut self.selection {
            *end = point;
        }
        self.dirty = true;
    }
    pub fn selected(&self) -> String {
        let Some((a, b)) = self.selection else {
            return String::new();
        };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        self.parser.screen().contents_between(a.0, a.1, b.0, b.1)
    }
    pub fn history(&mut self, limit: usize) -> String {
        let screen = self.parser.screen_mut();
        let original = screen.scrollback();
        let (rows, cols) = screen.size();
        let history = screen.scrollback_len();
        let total = history + usize::from(rows.get());
        let mut start = total.saturating_sub(limit);
        let mut text = String::new();
        while start < total {
            let offset = history.saturating_sub(start);
            screen.set_scrollback(offset);
            let first = start.saturating_sub(history) as u16;
            let lines: Vec<_> = screen.rows(0, cols.get()).collect();
            for row in first..rows.get() {
                text.push_str(&lines[row as usize]);
                if !screen.row_wrapped(row) {
                    text.push('\n');
                }
                start += 1;
                if start >= total {
                    break;
                }
            }
        }
        screen.set_scrollback(original);
        text.trim_end_matches('\n').into()
    }
    pub fn find(&mut self, query: &str, previous: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        if self.find_query != query {
            self.find_position = None;
            self.find_query = query.into();
        }
        let query = query.to_lowercase();
        let screen = self.parser.screen_mut();
        let old = screen.scrollback();
        let (rows, cols) = screen.size();
        let history = screen.scrollback_len();
        let total = history + rows.get() as usize;
        let mut start = 0usize;
        let mut found = vec![];
        while start < total {
            screen.set_scrollback(history.saturating_sub(start));
            let first = start.saturating_sub(history) as u16;
            for row in first..rows.get() {
                let mut text = String::new();
                let mut mapping = vec![];
                for col in 0..cols.get() {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let chars = if cell.contents().is_empty() {
                        " ".to_owned()
                    } else {
                        cell.contents().to_lowercase()
                    };
                    mapping.extend(std::iter::repeat_n(
                        (col, col + if cell.is_wide() { 2 } else { 1 }),
                        chars.len(),
                    ));
                    text.push_str(&chars);
                }
                for (at, _) in text.match_indices(&query) {
                    found.push((start, mapping[at].0, mapping[at + query.len() - 1].1));
                }
                start += 1;
                if start >= total {
                    break;
                }
            }
        }
        let selected = if let Some(position) = self.find_position {
            if previous {
                found
                    .iter()
                    .rev()
                    .find(|(row, col, _)| (*row, *col) < position)
                    .or_else(|| found.last())
            } else {
                found
                    .iter()
                    .find(|(row, col, _)| (*row, *col) > position)
                    .or_else(|| found.first())
            }
        } else {
            found.last()
        };
        let Some(&(absolute, col, end)) = selected else {
            screen.set_scrollback(old);
            return false;
        };
        let offset = history.saturating_sub(absolute);
        screen.set_scrollback(offset);
        let row = absolute.saturating_sub(history - offset) as u16;
        self.selection = Some(((row, col), (row, end)));
        self.find_position = Some((absolute, col));
        self.dirty = true;
        true
    }

    pub fn frame(&mut self) -> Vec<TerminalCell> {
        self.dirty = false;
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut cells: Vec<TerminalCell> = Vec::new();
        let palette = PALETTES[self.theme];
        let background = rgb(palette[0]);
        for row in 0..rows.get() {
            let mut highlights = vec![None; cols.get() as usize];
            if self.highlighting && !screen.alternate_screen() {
                let mut text = String::new();
                let mut mapping = vec![];
                for col in 0..cols.get() {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let chars = if cell.contents().is_empty() {
                        " "
                    } else {
                        cell.contents()
                    };
                    let own = cell.fgcolor() != vt100::Color::Default
                        || cell.bgcolor() != vt100::Color::Default
                        || cell.inverse();
                    mapping.extend(std::iter::repeat_n(
                        (col, if cell.is_wide() { 2 } else { 1 }, own),
                        chars.len(),
                    ));
                    text.push_str(chars);
                }
                for (start, end, kind) in crate::highlight::find(&text) {
                    if mapping[start..end].iter().any(|(_, _, own)| *own) {
                        continue;
                    }
                    let (from, _, _) = mapping[start];
                    let (to, width, _) = mapping[end - 1];
                    for col in from..to + width {
                        highlights[col as usize] = Some(kind);
                    }
                }
            }
            for col in 0..cols.get() {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let foreground = match cell.fgcolor() {
                    vt100::Color::Idx(index) if index < 8 && cell.bold() => {
                        vt100::Color::Idx(index + 8)
                    }
                    other => other,
                };
                let mut fg = color(foreground, rgb(palette[1]), &palette);
                let mut bg = color(cell.bgcolor(), background, &palette);
                if cell.inverse() ^ screen.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.dim() {
                    fg = Color::from_rgb_u8(fg.red() / 2, fg.green() / 2, fg.blue() / 2);
                }
                if let Some(kind) = highlights[col as usize] {
                    fg = self.highlight_colors[kind];
                }
                if let Some((a, b)) = self.selection {
                    let (a, b) = if a <= b { (a, b) } else { (b, a) };
                    if (row, col) >= a && (row, col) < b {
                        let (r, g, b, alpha) = if self.theme == 0 {
                            (58, 111, 192, 0.2)
                        } else if self.theme == 3 {
                            (147, 186, 245, 0.24)
                        } else {
                            (136, 179, 238, if self.theme == 2 { 0.22 } else { 0.24 })
                        };
                        let mix = |a: u8, b: u8| {
                            ((f64::from(a) * (1.0 - alpha)) + (f64::from(b) * alpha)) as u8
                        };
                        bg = Color::from_rgb_u8(
                            mix(bg.red(), r),
                            mix(bg.green(), g),
                            mix(bg.blue(), b),
                        );
                    }
                }
                let text = if cell.contents().is_empty() {
                    " "
                } else {
                    cell.contents()
                };
                if text == " " && bg == background && !cell.underline() {
                    continue;
                }
                let span = if cell.is_wide() { 2 } else { 1 };
                // Merge only ASCII: each Unicode grapheme keeps its exact cell position.
                if text.is_ascii()
                    && let Some(last) = cells.last_mut()
                    && last.row == i32::from(row)
                    && last.col + last.span == i32::from(col)
                    && last.text.is_ascii()
                    && last.foreground == fg
                    && last.background == bg
                    && last.bold == cell.bold()
                    && last.underline == cell.underline()
                {
                    last.text.push_str(text);
                    last.span += span;
                    continue;
                }
                cells.push(TerminalCell {
                    row: row.into(),
                    col: col.into(),
                    span,
                    text: text.into(),
                    foreground: fg,
                    background: bg,
                    bold: cell.bold(),
                    underline: cell.underline(),
                });
            }
        }
        cells
    }
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).unwrap()
}

fn rgb(value: u32) -> Color {
    Color::from_rgb_u8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn color(value: vt100::Color, default: Color, palette: &[u32; 18]) -> Color {
    match value {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color::from_rgb_u8(r, g, b),
        vt100::Color::Idx(index) if index < 16 => rgb(palette[index as usize + 2]),
        vt100::Color::Idx(index) if index >= 232 => {
            let value = 8 + (index - 232) * 10;
            Color::from_rgb_u8(value, value, value)
        }
        vt100::Color::Idx(index) => {
            let n = index - 16;
            let level = |v| if v == 0 { 0 } else { 55 + v * 40 };
            Color::from_rgb_u8(level(n / 36), level(n / 6 % 6), level(n % 6))
        }
    }
}

/// Encode Slint keys in the terminal's current cursor mode (including xterm modifiers).
pub fn encode_key(text: &str, control: bool, alt: bool, shift: bool, application: bool) -> Vec<u8> {
    use slint::platform::Key;
    let mut chars = text.chars();
    let Some(ch) = chars.next() else {
        return Vec::new();
    };
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    let arrows = [
        Key::UpArrow,
        Key::DownArrow,
        Key::RightArrow,
        Key::LeftArrow,
        Key::Home,
        Key::End,
    ];
    if let Some(i) = arrows.iter().position(|k| char::from(*k) == ch) {
        let suffix = ['A', 'B', 'C', 'D', 'H', 'F'][i];
        return if modifier > 1 {
            format!("\x1b[1;{modifier}{suffix}")
        } else {
            format!("\x1b{}{suffix}", if application { 'O' } else { '[' })
        }
        .into_bytes();
    }
    let numbered = [
        Key::Insert,
        Key::Delete,
        Key::PageUp,
        Key::PageDown,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    if let Some(i) = numbered.iter().position(|k| char::from(*k) == ch) {
        let number = [2, 3, 5, 6, 15, 17, 18, 19, 20, 21, 23, 24][i];
        return if modifier > 1 {
            format!("\x1b[{number};{modifier}~")
        } else {
            format!("\x1b[{number}~")
        }
        .into_bytes();
    }
    if let Some(i) = [Key::F1, Key::F2, Key::F3, Key::F4]
        .iter()
        .position(|k| char::from(*k) == ch)
    {
        let suffix = ['P', 'Q', 'R', 'S'][i];
        return if modifier > 1 {
            format!("\x1b[1;{modifier}{suffix}")
        } else {
            format!("\x1bO{suffix}")
        }
        .into_bytes();
    }
    if matches!(ch, '\u{10}'..='\u{18}' | '\u{f700}'..='\u{f8ff}') {
        return Vec::new();
    }
    let result = match ch {
        '\n' | '\r' => vec![b'\r'],
        '\x08' => vec![0x7f],
        '\x19' => b"\x1b[Z".to_vec(),
        '\t' if shift => b"\x1b[Z".to_vec(),
        ' ' | '@' if control => vec![0],
        '?' if control => vec![0x7f],
        _ if control && ch.is_ascii() && ('@'..='_').contains(&ch.to_ascii_uppercase()) => {
            vec![ch.to_ascii_uppercase() as u8 & 0x1f]
        }
        _ => text.as_bytes().to_vec(),
    };
    if alt {
        [b"\x1b".as_slice(), &result].concat()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_handles_wide_characters_and_search_reaches_history() {
        let mut terminal = Terminal::with_scrollback(2, 20, 1000);
        terminal.process("Я界 abc\r\nsecond\r\nthird\r\nfourth".as_bytes());
        let live = terminal.parser.screen().contents();
        assert!(terminal.find("Я界", false));
        assert_eq!(terminal.selected(), "Я界");
        assert!(terminal.parser.screen().scrollback() > 0);
        assert!(terminal.find("third", false));
        assert_eq!(terminal.selected(), "third");
        let offset = terminal.parser.screen().scrollback();
        assert_eq!(terminal.history(4), "Я界 abc\nsecond\nthird\nfourth");
        assert_eq!(terminal.parser.screen().scrollback(), offset);
        terminal.follow_output();
        assert_eq!(terminal.parser.screen().contents(), live);
        assert!(!terminal.find("missing", false));
        assert_eq!(terminal.parser.screen().scrollback(), 0);
    }

    #[test]
    fn highlighting_preserves_program_colours_and_fullscreen_apps() {
        let mut terminal = Terminal::new(3, 60);
        terminal.highlighting = true;
        terminal.highlight_colors = [Color::from_rgb_u8(1, 2, 3); 6];
        terminal.process(b"ERROR \x1b[31mERROR\x1b[0m errorCount");
        let cells = terminal.frame();
        assert_eq!(cells[0].foreground, Color::from_rgb_u8(1, 2, 3));
        assert!(cells.iter().any(|c| c.text == "ERROR"
            && c.col == 6
            && c.foreground != Color::from_rgb_u8(1, 2, 3)));
        terminal.process(b"\x1b[?1049hERROR");
        assert!(
            terminal
                .frame()
                .iter()
                .all(|c| c.foreground != Color::from_rgb_u8(1, 2, 3))
        );
    }

    #[test]
    fn changing_theme_recolors_existing_ansi_output_and_preserves_truecolor() {
        let mut terminal = Terminal::new(4, 40);
        terminal.process(b"\x1b[31mred\x1b[38;2;17;34;51mfixed");
        let light = terminal.frame();
        terminal.set_theme(1);
        assert!(terminal.dirty);
        let dark = terminal.frame();
        assert_ne!(light[0].foreground, dark[0].foreground);
        assert_ne!(light[0].background, dark[0].background);
        assert_eq!(light[1].foreground, Color::from_rgb_u8(17, 34, 51));
        assert_eq!(light[1].foreground, dark[1].foreground);
        assert_eq!(terminal.parser.screen().contents(), "redfixed");
    }

    #[test]
    fn screen_tracks_escape_sequences_utf8_and_wide_cells() {
        let mut term = Terminal::new(4, 20);
        term.process(b"old\r\x1b[2K\x1b[38;2;20;40;60mnew");
        // Split UTF-8 over chunks, as a network connection can.
        let unicode = "Я界".as_bytes();
        term.process(&unicode[..1]);
        term.process(&unicode[1..]);
        let frame = term.frame();
        assert_eq!(term.parser.screen().contents(), "newЯ界");
        assert_eq!(frame[0].foreground, Color::from_rgb_u8(20, 40, 60));
        assert!(
            frame
                .iter()
                .any(|cell| cell.text == "界" && cell.col == 4 && cell.span == 2)
        );
        term.process(b"\x1b[?1049hother\x1b[?1049l");
        assert_eq!(term.parser.screen().contents(), "newЯ界");
    }

    #[test]
    fn answers_terminal_queries_even_when_split_between_chunks() {
        let mut term = Terminal::new(24, 80);
        term.process(b"\x1b[3;9H\x1b[");
        term.process(b"6n\x1b[5n\x1b[c");
        assert_eq!(term.replies(), b"\x1b[3;9R\x1b[0n\x1b[?1;2c");
        assert!(term.replies().is_empty());
    }

    #[test]
    fn paste_cannot_inject_an_escape_to_end_bracketing() {
        let mut term = Terminal::new(24, 80);
        assert_eq!(term.paste("a\r\nb\x03\x1b[201~"), b"a\rb[201~");
        term.process(b"\x1b[?2004h");
        assert_eq!(term.paste("a\nb"), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn navigation_respects_modes_and_modifiers() {
        use slint::platform::Key;
        let up = char::from(Key::UpArrow).to_string();
        assert_eq!(encode_key(&up, false, false, false, true), b"\x1bOA");
        assert_eq!(encode_key(&up, true, false, false, true), b"\x1b[1;5A");
        assert_eq!(encode_key("c", true, false, false, false), [3]);
        assert_eq!(encode_key("\n", false, false, false, false), b"\r");
        assert_eq!(encode_key("\x08", false, false, false, false), [127]);
    }

    #[test]
    fn scrollback_and_resize_keep_a_bounded_screen() {
        let mut term = Terminal::new(2, 10);
        term.process(b"one\r\ntwo\r\nthree");
        term.scroll(1);
        assert!(term.parser.screen().contents().contains("one"));
        term.follow_output();
        assert_eq!(term.parser.screen().scrollback(), 0);
        assert!(term.resize(3, 12));
        assert!(!term.resize(3, 12));
    }
}
