use std::io::{self, BufWriter, Write, Read};
use std::os::unix::prelude::{AsRawFd, RawFd, FromRawFd, OwnedFd};


use crate::{Candidate, other_error};
use crate::filter::{self, Range};

#[derive(Debug, Clone, Copy)]
pub enum Attributes {
    Reset,
    Reverse,
    FgCyan,
    FgDefault,
}

impl Attributes {
    pub fn tou8(&self) -> u8 {
        use Attributes::*;

        match self {
            Reset     => 0,
            Reverse   => 7,
            FgCyan    => 36,
            FgDefault => 39,
        }
    }
}

pub struct Terminal {
    owned_fd: OwnedFd,
    reader: std::fs::File,
    writer: BufWriter<std::fs::File>,

    term: termios::Termios,
    raw_term: termios::Termios,
    max_height: usize,
    height: usize,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(self.rawfd(), termios::TCSANOW, &self.term).expect("fail to reset optons");
        println!("finish closing file...");
    }
}

impl Terminal {
    fn rawfd(&self) -> RawFd {
        self.owned_fd.as_raw_fd()
    }

    pub fn clean_up(&mut self) -> io::Result<()> {
        for _ in 0..self.height {
            self.clean_line()?;
            self.cursor_down(1)?;
        }
        self.clear_line()?;
        self.cursor_up(self.height)?;
        self.writer.flush()?;

        Ok(())
    }

    pub fn new(max_height: usize) -> io::Result<Self> {
        let owned_fd = OwnedFd::from(std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty")?);
        let fd = owned_fd.as_raw_fd();
        let writer = BufWriter::new(unsafe { std::fs::File::from_raw_fd(fd) });
        let reader = unsafe { std::fs::File::from_raw_fd(fd) };

        let term = termios::Termios::from_fd(fd)?;
        let mut raw_term = term;

        raw_term.c_iflag &= !(termios::ICRNL);
        raw_term.c_lflag &= !(termios::ICANON | termios::ECHO | termios::ISIG);

        termios::tcsetattr(fd, termios::TCSANOW, &raw_term)?;

        Ok(Self { owned_fd, reader, writer, term, raw_term, max_height, height: 0, })
    }

    // ensure enough room to draw all lines of output by drawing blank lines,
    // effectively scrolling the view. + 1 to also include the prompt's offset
    pub fn run(&mut self, candidates: Vec<Candidate>, keep_order: bool) -> io::Result<usize> {
        let mut query: String = String::new();
        let mut state = State::default();

        self.determine_height()?;
        self.scroll_down(self.height)?;
        self.cursor_up(self.height)?;

        let mut filtered = candidates.clone();
        let mut old_state = state.clone();
        let mut old_query = query.clone();

        let mut redraw = true;
        loop {
            // did the query change?
            if query != old_query {
                if query.len() == 0 {
                    filtered = candidates.clone();
                } else {
                    filtered = filter::rank_candidates(candidates.clone(), &query, keep_order);
                    redraw = true;
                    state.selected = 0;
                    old_query = query.clone();
                }
            }

            // did the selection move?
            if redraw || state.cursor != old_state.cursor || state.selected != old_state.selected {
                self.draw(&state, &query, &filtered, candidates.len())?;
                old_state = state.clone();
                redraw = false;
            }

            let visible_rows = self.height.min(filtered.len());
            if let Some(key) = self.read_key() {
                match key_to_action(key) {
                    Action::Close => break,
                    Action::Byte(byte) => {
                        assert!(state.cursor <= query.len(), "internal error");
                        query.insert(state.cursor, byte as char);
                        state.cursor += 1;
                    },
                    Action::DeleteWord => {
                        action_delete_word(&mut query, &mut state.cursor);
                    },
                    Action::Backspace => {
                        if query.len() > 0 && state.cursor == query.len() {
                            query.pop();
                            state.cursor -= 1;
                        } else if query.len() > 0 && state.cursor > 0 {
                            query.remove(state.cursor);
                            state.cursor -= 1;
                        }
                    }
                    Action::Pass => { },
                    _ => {  }
                }
            }
        }

        Ok(0)
    }

    fn draw_candidate(&mut self, candidate: &Candidate, width: usize, selected: bool) -> io::Result<()> {
        let out: io::Result<()> = {
            if selected { self.sgr(Attributes::Reverse)?; }
            let path = shrink_str(&candidate.path, width);

            if candidate.ranges.len() == 0 {
                self.writer.write(path.as_bytes())?;
            } else {
                // self.writer.write(format!("{:?}", candidate.ranges).as_bytes())?;
                for (flag, slice) in IterRanges::new(candidate.ranges.iter(), path.len()) {
                    let segment = &path[slice.start..slice.end];
                    if flag {
                        self.sgr(Attributes::FgCyan)?;
                    } else {
                        self.sgr(Attributes::FgDefault)?;
                    }

                    // self.writer.write(format!("{:?} ", slice).as_bytes())?;
                    self.writer.write(segment.as_bytes())?;
                }
            }

            Ok(())
        };

        self.sgr(Attributes::Reset)?;
        out
    }

    fn draw(&mut self, state: &State, query: &str, candidates: &[Candidate], len: usize) -> io::Result<()> {
        let width = self.window_size()?.x;

        // draw the candidates
        for line in 0..self.height {
            self.cursor_down(1)?;
            self.clear_line()?;
            if line < candidates.len() {
                self.draw_candidate(&candidates[line], width, line == state.selected)?;
            }
        }
        self.sgr(Attributes::Reset)?;
        self.cursor_up(self.height)?;

        // draw the prompt
        {
            self.clear_line()?;
            self.writer.write(b"> ")?;
            self.writer.write(shrink_str(query, width.saturating_sub(2)).as_bytes())?;
        }

        // draw info if there is room
        let prompt_width = 2;
        let info_str = format!("{}/{}", candidates.len(), len);
        let spacing = width.saturating_sub(
            prompt_width + query.len() + info_str.len()
        );

        if spacing >= 1 {
            self.cursor_right(spacing)?;
            self.writer.write(info_str.as_bytes())?;
        }

        // position the cursor at the edit location
        self.cursor_col(1)?;
        self.cursor_right((width-1).min(state.cursor+2))?;

        self.writer.flush()
    }

    fn write(&mut self, num: usize, chr: char) -> io::Result<()> {
        self.writer.write(b"\x1b[")?;
        self.writer.write(num.to_string().as_bytes())?;
        self.writer.write(&[chr as u8])?;
        Ok(())
    }

    fn cursor_up(&mut self, num: usize) -> io::Result<()> {
        self.write(num, 'A')
    }

    fn cursor_col(&mut self, col: usize) -> io::Result<()> {
        self.write(col, 'G')
    }

    fn cursor_down(&mut self, num: usize) -> io::Result<()> {
        self.write(num, 'B')
    }

    fn cursor_right(&mut self, num: usize) -> io::Result<()> {
        self.write(num, 'C')
    }

    fn cursor_left(&mut self, num: usize) -> io::Result<()> {
        self.write(num, 'D')
    }

    fn clear_line(&mut self) -> io::Result<()> {
        self.cursor_col(1)?;
        self.write(2, 'K')
    }

    fn scroll_down(&mut self, num: usize) -> io::Result<()> {
        for _ in 0..num {
            self.writer.write(b"\n")?;
        }
        Ok(())
    }

    fn clean_line(&mut self) -> io::Result<()> {
        self.cursor_col(1)?;
        self.write(22, 'K')?;
        Ok(())
    }

    fn sgr(&mut self, code: Attributes) -> io::Result<()> {
        self.write(code.tou8() as usize,  'm')
    }

    fn nodelay(&mut self, state: bool) -> io::Result<()> {
        self.raw_term.c_cc[termios::os::linux::VMIN] = if state { 0 } else { 1 };
        termios::tcsetattr(self.rawfd(), termios::TCSANOW, &self.raw_term)?;
        Ok(())
    }

    fn window_size(&self) -> io::Result<WinSize> {
        unsafe {
            let mut win_size: libc::winsize = std::mem::zeroed();
            if libc::ioctl(self.rawfd(), libc::TIOCGWINSZ, &mut win_size ) != 0 {
                return Err(other_error("ioctl call failed"));
            }
            Ok(WinSize { x: win_size.ws_col as _, y: win_size.ws_row as _ })
        }
    }

    fn determine_height(&mut self) -> io::Result<()> {
        let win_size = self.window_size()?;
        self.height = self.max_height.clamp(1, win_size.y - 1);
        Ok(())
    }

    // *block* until read a key or timeout(return None)
    pub fn read_key(&mut self) -> Option<Key> {
        let mut byte: u8 = 0;
        if let Ok(_) = self.reader.read_exact(std::slice::from_mut(&mut byte)) {
            if byte == ('\x1b' as u8) {
                self.nodelay(true).ok()?;
                let mut seq = [0; 2];
                let out = self.reader.read_exact(&mut seq);
                self.nodelay(false).ok()?;
                if out.is_err() { return None; }

                // DECCKM mode sends \x1bO* instead of \x1b[*
                if matches!(seq[0] as char, 'O' | '[') {
                    return match seq[1] as char {
                        'A' => Some(Key::Up),
                        'B' => Some(Key::Down),
                        'C' => Some(Key::Right),
                        'D' => Some(Key::Left),
                        '3' => Some(read_delete(&self.reader)),
                        _ => Some(Key::Esc)
                    }
                }
                return Some(Key::Esc)
            }

            if byte == '\r' as u8 {
                return Some(Key::Enter);
            } else if byte == 127 {
                return Some(Key::Backspace)
            }

            if unsafe { libc::iscntrl(byte as _) } > 0 {
                return Some(Key::Control(byte));
            }

            if unsafe { libc::isprint(byte as _) } > 0 {
                return Some(Key::Character(byte));
            }

            return Some(Key::Esc)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Character(u8),
    Control(u8),
    Esc,
    Up,
    Down,
    Left,
    Right,
    Backspace,
    Delete,
    Enter,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Action {
    Byte(u8),
    LineUp,
    LineDown,
    CursorLeft,
    CursorLeftMost,
    CursorRight,
    CursorRightMost,
    Backspace,
    Delete,
    DeleteWord,
    DeleteLine,
    Select,
    Close,
    Pass,
}

const fn ctrl(ch: char) -> u8 {
    (ch as u8) & 0x1f
}

fn ctrl_to_action(key: u8) -> Action {
    match key {
        c if c == ctrl('c') => Action::Close,
        c if c == ctrl('w') => Action::DeleteWord,
        c if c == ctrl('u') => Action::DeleteLine,
        c if c == ctrl('h') => Action::Backspace,
        c if c == ctrl('a') => Action::CursorLeftMost,
        c if c == ctrl('e') => Action::CursorRightMost,
        c if c == ctrl('d') => Action::Delete,
        c if c == ctrl('f') => Action::CursorRight,
        c if c == ctrl('b') => Action::CursorLeft,
        c if c == ctrl('p') || c == ctrl('k') => Action::LineUp,
        c if c == ctrl('n') || c == ctrl('j') => Action::LineDown,
        _ => Action::Pass,
    }
}

fn key_to_action(key: Key) -> Action {
    match key {
        Key::Character(c) => Action::Byte(c),
        Key::Control(c) => ctrl_to_action(c),
        Key::Backspace => Action::Backspace,
        Key::Delete => Action::Delete,
        Key::Up => Action::LineUp,
        Key::Down => Action::LineDown,
        Key::Left => Action::CursorLeft,
        Key::Right => Action::CursorRight,
        Key::Enter => Action::Select,
        Key::Esc => Action::Close,
    }

}

fn read_delete<R: Read>(mut file: R) -> Key {
    let mut byte: u8 = 0;
    if let Ok(_) = file.read_exact(&mut std::slice::from_mut(&mut byte)) {
        if byte as char == '~' {
            return Key::Delete;
        }
    }
    Key::Esc
}

#[derive(Debug, Clone, Default, Copy)]
struct State {
    pub cursor: usize,
    pub selected: usize,
}

#[derive(Debug, Clone, Default)]
struct WinSize {
    x: usize,
    y: usize,
}

fn shrink_str(s: &str, width: usize) -> &str {
    let mut last_width = 0;
    for (idx, chr) in s.chars().enumerate() {
        if idx > width {

            return &s[..last_width];
        }
        last_width += chr.len_utf8();
    }
    &s[..last_width]
}

struct IterRanges<I> {
    iter: I,
    stop: usize,

    last: Option<Range>,
    start: usize,
}

impl<'r, I: Iterator<Item=&'r Range>> IterRanges<I> {
    fn new(mut iter: I, stop: usize) -> Self {
        let last = iter.next().map(Clone::clone);
        Self {
            iter,
            stop,

            last,
            start: 0,
        }
    }
}

impl<'r, I: Iterator<Item=&'r Range>> Iterator for IterRanges<I> {
    type Item = (bool, Range);
    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.stop { return None; }
        if let Some(Range { start, mut end }) = self.last {
            end = end + 1;
            debug_assert!(self.start <= start);
            debug_assert!(start < end, "start: {}, end: {}", start, end);
            end = end.min(self.stop);
            let flag; let out;

            if self.start == start {
                flag = true;
                out = Range { start, end };

                self.last = self.iter.next().map(Clone::clone);
                self.start = end;
            } else {
                flag = false;
                out = Range { start: self.start, end: start };
                self.start = start;
            };

            Some((flag, out))
        } else {
            if self.start < self.stop {
                let out = Range { start: self.start, end: self.stop};
                self.start = self.stop;
                Some((false, out))
            } else {
                None
            }
        }
    }
}

fn action_delete_word(query: &str, cursor: &mut usize) {


}
