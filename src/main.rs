use core::panic;
use crossterm::cursor;
use crossterm::cursor::MoveTo;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use crossterm::event::{Event, KeyCode, read};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use ropey::Rope;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::stdout;
use std::io::{BufReader, Write};
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Mutex;
use std::usize;
use std::vec;
use std::{env, io};

impl From<std::io::Error> for EditorErr {
    fn from(e: std::io::Error) -> Self {
        EditorErr::Io(e)
    }
}
#[derive(Debug)]
enum EditorErr {
    Io(std::io::Error),
    ReadOnly(BufferIdx),
    InvalidBuffer,
    InvalidFocus,
    Dirty(BufferIdx),
    Msg(String),
    Log(String),
    Quit,
}
struct Logger {
    file: &'static str,
}
impl Logger {
    fn log(&self, msg: &str) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .expect("failed to open log file");
        writeln!(file, "{}", msg).expect("failed to write log");
    }
}
fn log(msg: &str) {
    LOGGER.lock().unwrap().log(msg);
}
static LOGGER: Mutex<Logger> = Mutex::new(Logger { file: "log" });

pub fn yank_to_system_clipboard(text: &str) -> io::Result<()> {
    let text = text.strip_suffix("\n").unwrap_or(text);
    #[cfg(target_os = "linux")]
    {
        // helper to try a command
        fn try_cmd(program: &str, args: &[&str], text: &str) -> io::Result<()> {
            let mut child = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .spawn()?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Failed to open stdin"))?
                .write_all(text.as_bytes())?;

            let status = child.wait()?;
            if status.success() {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::Other, "Command failed"))
            }
        }

        try_cmd("wl-copy", &[], text)
            .or_else(|_| try_cmd("xclip", &["-selection", "clipboard"], text))
            .or_else(|_| try_cmd("xsel", &["--clipboard", "--input"], text))?;
    }

    #[cfg(target_os = "macos")]
    {
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;

        child.stdin.as_mut().unwrap().write_all(text.as_bytes())?;
        child.wait()?;
    }

    #[cfg(target_os = "windows")]
    {
        let mut child = Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(Stdio::piped())
            .spawn()?;

        child.stdin.as_mut().unwrap().write_all(text.as_bytes())?;
        child.wait()?;
    }

    Ok(())
}
fn paste_to_system_clipboard() -> io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let commands = [
            ("wl-paste", &[][..]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ];

        for (cmd, args) in commands {
            if let Ok(output) = Command::new(cmd).args(args).output() {
                if output.status.success() {
                    return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No clipboard tool found (wl-paste, xclip, xsel)",
        ))
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("pbpaste").output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "pbpaste failed"))
        }
    }

    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard"])
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "PowerShell Get-Clipboard failed",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferIdx {
    idx: usize,
}
struct Buffers {
    data: Vec<Buffer>,
    free: VecDeque<BufferIdx>,
    path_map: HashMap<PathBuf, BufferIdx>,
}
impl Buffers {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            free: VecDeque::new(),
            path_map: HashMap::new(),
        }
    }
    fn get(&self, idx: BufferIdx) -> &Buffer {
        &self.data[idx.idx]
    }
    fn get_mut(&mut self, idx: BufferIdx) -> &mut Buffer {
        &mut self.data[idx.idx]
    }
    fn push(&mut self, buf: Buffer) -> BufferIdx {
        let path = buf.file.clone();
        let idx = if self.free.is_empty() {
            let idx = BufferIdx {
                idx: self.data.len(),
            };
            self.data.push(buf);
            idx
        } else {
            let idx = self.free.pop_front().unwrap();
            let element = self.get_mut(idx);
            *element = buf;
            idx
        };
        if let Some(p) = path {
            if let Ok(path) = p.canonicalize() {
                self.path_map.insert(path, idx);
            }
        }
        idx
    }
    fn get_by_path(&self, path: &str) -> Option<&BufferIdx> {
        if let Ok(p) = Path::new(path).canonicalize() {
            let buffer = self.path_map.get(&p);
            if let Some(idx) = buffer {
                return Some(idx);
            }
        }
        None
    }
    fn remove(&mut self, idx: &mut BufferIdx) {
        self.get_mut(*idx).generation += 1;
        self.data[idx.idx].partial_reset();
        self.free.push_back(*idx);
    }
    fn len(&self) -> usize {
        self.data.len()
    }
    fn iter(&self) -> impl Iterator<Item = &Buffer> {
        self.data.iter()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewIdx(usize);
struct Views(Vec<View>);
impl Views {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn get(&self, idx: ViewIdx) -> &View {
        &self.0[idx.0]
    }
    fn get_mut(&mut self, idx: ViewIdx) -> &mut View {
        &mut self.0[idx.0]
    }
    fn push(&mut self, view: View) -> ViewIdx {
        let idx = ViewIdx(self.0.len());
        self.0.push(view);
        idx
    }
}

#[derive(Clone, Copy)]
struct Constraints {
    min_height: Option<u16>,
    max_height: Option<u16>,
    min_width: Option<u16>,
    max_width: Option<u16>,
}

#[derive(Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    height: u16,
    width: u16,
    constraint: Constraints,
}

enum Edit {
    Insert { idx: usize, text: String },
    Delete { idx: usize, text: String },
}
struct Buffer {
    generation: u64,
    flags: u64,
    file: Option<PathBuf>,
    buf: Rope,
    last_off: usize,
    last_cursor: usize,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl Buffer {
    const READ_ONLY: u64 = 1 << 0;
    const SCRATCH: u64 = 1 << 1;
    const NEW_FILE: u64 = 1 << 2;
    const NON_NAVIGATABLE: u64 = 1 << 3;
    fn partial_reset(&mut self) {
        self.buf = Rope::new();
        self.undo = Vec::new();
        self.redo = Vec::new();
        //intentional does not reset flags or pathbuf
    }
    fn set_flag(&mut self, flag: u64) {
        self.flags |= flag
    }
    fn clear_flag(&mut self, flag: u64) {
        self.flags &= !flag
    }
    fn check_flag(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }
    fn new(path: Option<&str>, flags: u64) -> std::io::Result<Buffer> {
        let mut f = flags;
        let buf = if let Some(p) = path {
            let path = PathBuf::from(p);
            if path.exists() {
                let cont = fs::read_to_string(&path)?;
                if fs::metadata(&path)?.permissions().readonly() {
                    f |= Self::READ_ONLY;
                }
                Rope::from_str(&cont)
            } else {
                f |= Self::NEW_FILE;
                Rope::new()
            }
        } else {
            f |= Self::NEW_FILE;
            Rope::new()
        };
        Ok(Buffer {
            generation: 0,
            flags: f,
            buf: buf,
            last_off: 0,
            last_cursor: 0,
            file: path.map(PathBuf::from),
            redo: Vec::new(),
            undo: Vec::new(),
        })
    }
    fn insert(&mut self, off: usize, cursor: usize, c: char) {
        self.last_cursor = cursor;
        self.last_off = off;
        self.buf.insert_char(cursor, c);
    }
    fn insert_string(&mut self, off: usize, cursor: usize, s: &str) {
        self.last_cursor = cursor;
        self.last_off = off;
        for c in s.chars().rev() {
            self.buf.insert_char(cursor, c);
        }
    }
    fn save(&mut self, new: Option<String>) -> io::Result<()> {
        if let Some(new) = new {
            let file = File::create(new)?;
            self.buf.write_to(file)?;
        } else {
            if let Some(path) = &self.file {
                let file = File::create(path)?;
                self.buf.write_to(file)?;
            }
        }
        Ok(())
    }
}

struct CmdLine {
    input: String,
    rect: Rect,
    cursor: usize,
    error: bool,
    last_view: (LeafIdx, ViewIdx),
}
impl CmdLine {
    fn new(width: u16, height: u16) -> Self {
        Self {
            input: String::new(),
            rect: Rect {
                x: 0,
                y: height,
                height: 1,
                width: width,
                constraint: Constraints {
                    min_height: Some(1),
                    min_width: Some(width),
                    max_height: Some(1),
                    max_width: Some(width),
                },
            },
            cursor: 0,
            error: false,
            last_view: (LeafIdx(usize::MAX), ViewIdx(usize::MAX)),
        }
    }
    fn enter_cmd_mode(
        &mut self,
        vidx: ViewIdx,
        focus: &mut Focus,
        views: &mut Views,
        lidx: LeafIdx,
    ) {
        self.last_view = (lidx, vidx);
        views.get_mut(vidx).mode = Mode::Normal;
        self.input.clear();
        self.cursor = 0;
        *focus = Focus::CmdLine;
        queue!(stdout(), cursor::SetCursorStyle::SteadyBar).unwrap();
    }
    fn insert(&mut self, c: char) {
        if self.error {
            self.cursor = 0;
            self.input.clear();
            self.error = false;
        }
        let byte_idx = self.cursor;
        self.input.insert(byte_idx, c);
        self.cursor += c.len_utf8();
    }
    fn backspace(&mut self) {
        if self.error {
            self.cursor = 0;
            self.input.clear();
            self.error = false;
        }
        if self.cursor > 0 {
            let char_len = self.input[..self.cursor]
                .chars()
                .rev()
                .next()
                .unwrap()
                .len_utf8();
            self.cursor -= char_len as usize;
            self.input.remove(self.cursor);
        }
    }
    fn error(&mut self, s: &str) {
        self.error = true;
        self.input.clear();
        self.input = s.to_string();
    }
    fn sketch(&self, focus: &Focus, screen: &mut ScreenBuffer) {
        let s = {
            if self.error {
                format!("{}", self.input)
            } else {
                if let Focus::CmdLine = focus {
                    format!(":{}", self.input)
                } else {
                    if self.input.is_empty() {
                        "".to_string()
                    } else {
                        format!(":{}", self.input)
                    }
                }
            }
        };
        screen.set_string_xy(0, self.rect.y, &s, FG, BG);
    }
}

struct View {
    selection: Option<(usize, usize)>,
    clipboard: Option<String>,
    buf: BufferIdx,
    cursor: usize,
    prefered_x: usize,
    off: usize,
    mode: Mode,
}

impl View {
    fn new(buf: BufferIdx) -> Self {
        Self {
            buf,
            selection: None,
            clipboard: None,
            cursor: 0,
            prefered_x: 0,
            off: 0,
            mode: Mode::Normal,
        }
    }
    fn scroll(&mut self, rect: &Rect, buffer: &mut Buffer) {
        let line = buffer.buf.char_to_line(self.cursor);
        let height = {
            let mut wrap_off = 0usize;
            for line_idx in self.off..line {
                if let Some(line) = buffer.buf.get_line(line_idx) {
                    let len = line.len_chars();
                    if len > 0 {
                        wrap_off += len / rect.width.saturating_sub(5) as usize;
                    }
                }
            }
            let height = rect.height.saturating_sub(2) as usize;
            height.saturating_sub(wrap_off)
        };
        if line < self.off {
            self.off = line;
        } else if line > self.off + height {
            self.off = line - height;
        }
    }
}

const FG: Color = Color::White;
const BG: Color = Color::Rgb { r: 0, g: 0, b: 0 };
const SELECTION: Color = Color::Rgb {
    r: 20,
    g: 140,
    b: 240,
};

trait Component {
    fn sketch(&self, rect: &Rect, views: &Views, buffers: &Buffers, screen: &mut ScreenBuffer);
    fn cursor_xy(&self, rect: &Rect, views: &Views, buffers: &Buffers) -> (u16, u16);
    fn behaviour(
        &mut self,
        key: KeyEvent,
        focus: &mut Focus,
        cmd_line: &mut CmdLine,
        views: &mut Views,
        buffers: &mut Buffers,
        nodes: &mut Nodes,
    ) -> Result<(), EditorErr>;
}

impl Component for ViewIdx {
    fn sketch(&self, rect: &Rect, views: &Views, buffers: &Buffers, screen: &mut ScreenBuffer) {
        let v = views.get(*self);
        {
            let blank = " ".repeat(rect.width as usize);
            for row in 0..rect.height.saturating_sub(1) {
                screen.set_string_xy(rect.x, rect.y + row, &blank, FG, BG);
            }
        }
        deco_sketch(v, rect, buffers, screen);
        text_sketch(v, rect, buffers, screen);
        selection_sketch(v, rect, buffers, screen);
        fn text_sketch(view: &View, rect: &Rect, buffers: &Buffers, screen: &mut ScreenBuffer) {
            let width = rect.width.saturating_sub(5) as usize;
            let height = rect.height.saturating_sub(1) as usize;
            let mut row = 0;
            let mut line_idx = view.off;
            while row < height {
                let Some(line) = buffers.get(view.buf).buf.get_line(line_idx) else {
                    break;
                };
                let line_len = line.len_chars().saturating_sub(1); //remove trailing /n if not removed causes ghost words
                let mut start = 0usize;
                while start < line_len && row < height {
                    let end = usize::min(start + width, line_len);
                    let slice = line.slice(start..end);
                    screen.set_string_xy(
                        rect.x + 5,
                        rect.y + row as u16,
                        &slice.to_string(),
                        FG,
                        BG,
                    );
                    start = end;
                    row += 1;
                }
                if line_len == 0 {
                    row += 1;
                }
                line_idx += 1;
            }
        }
        fn deco_sketch(view: &View, rect: &Rect, buffers: &Buffers, screen: &mut ScreenBuffer) {
            let wrap_width = rect.width.saturating_sub(4) as usize;

            let mut screen_row = 0usize;
            let mut line_idx = view.off;

            while screen_row < rect.height as usize {
                let Some(line) = buffers.get(view.buf).buf.get_line(line_idx) else {
                    break;
                };

                let line_len = line.len_chars();
                let visual_rows = usize::max(1, line_len.div_ceil(wrap_width));

                for visual_row in 0..visual_rows {
                    if screen_row >= rect.height as usize {
                        break;
                    }

                    let screen_y = rect.y + screen_row as u16;

                    // only draw number on first wrapped row
                    let s = if visual_row == 0 {
                        format!("{:>4} ", line_idx)
                    } else {
                        "     ".to_string()
                    };

                    screen.set_string_xy(rect.x, screen_y, &s, FG, BG);

                    screen_row += 1;
                }

                line_idx += 1;
            }
            let mut path = "SCRATCH";
            let buffer = buffers.get(view.buf);
            if !buffer.check_flag(Buffer::SCRATCH) {
                if let Some(p) = &buffer.file {
                    path = p.to_str().unwrap_or("NEW_FILE");
                } else {
                    path = "NEW_FILE";
                }
            }
            let mode_str = match view.mode {
                Mode::Normal => "NOR",
                Mode::Insert => "INS",
                Mode::Visual => "VIS",
            };
            let s = format!("{mode_str} {} {path}", view.buf.idx);
            let s = format!("{:width$}", s, width = rect.width as usize);
            screen.set_string_xy(rect.x, rect.y + rect.height.saturating_sub(1), &s, FG, BG);
        }
        fn selection_sketch(
            view: &View,
            rect: &Rect,
            buffers: &Buffers,
            screen: &mut ScreenBuffer,
        ) {
            let Some((a, b)) = view.selection else {
                return;
            };
            let sel_start = usize::min(a, b);
            let sel_end = usize::max(a, b);
            let width = rect.width.saturating_sub(5) as usize;
            let height = rect.height.saturating_sub(1) as usize;
            let buffer = buffers.get(view.buf);
            let mut row = 0usize;
            let mut line_idx = view.off;
            let mut global_idx = 0usize;

            for i in 0..view.off {
                if let Some(line) = buffer.buf.get_line(i) {
                    global_idx += line.len_chars();
                }
            }
            while row < height {
                let Some(line) = buffer.buf.get_line(line_idx) else {
                    break;
                };
                let full_len = line.len_chars();
                let text_len = full_len.saturating_sub(1);
                if text_len == 0 {
                    let idx = global_idx;
                    if idx >= sel_start && idx < sel_end {
                        screen.set_cell_xy(
                            rect.x + 5,
                            rect.y + row as u16,
                            Cell {
                                c: ' ',
                                fg: BG,
                                bg: SELECTION,
                            },
                        );
                    }

                    row += 1;
                    global_idx += full_len;
                    line_idx += 1;
                    continue;
                }

                let mut start = 0usize;

                while start < text_len && row < height {
                    let end = usize::min(start + width, text_len);

                    for col in start..end {
                        let idx = global_idx + col;

                        if idx >= sel_start && idx < sel_end {
                            let c = line.char(col);

                            screen.set_cell_xy(
                                rect.x + 5 + (col - start) as u16,
                                rect.y + row as u16,
                                Cell {
                                    c,
                                    fg: BG,
                                    bg: SELECTION,
                                },
                            );
                        }
                    }

                    start = end;
                    row += 1;
                }

                // move past entire rope line INCLUDING newline
                global_idx += full_len;
                line_idx += 1;
            }
        }
    }
    fn cursor_xy(&self, rect: &Rect, views: &Views, buffers: &Buffers) -> (u16, u16) {
        let v = views.get(*self);
        let b = buffers.get(v.buf);
        let width = rect.width.saturating_sub(4) as usize;
        if width == 0 {
            return (rect.x + 5, rect.y);
        }
        let line = b.buf.char_to_line(v.cursor);
        let line_start = b.buf.line_to_char(line);
        let x = v.cursor - line_start;
        let y = x / width;
        let x = x % width;
        let mut nested_y = 0usize;
        for line_idx in v.off..line {
            if let Some(line) = b.buf.get_line(line_idx) {
                let len = line.len_chars();
                nested_y += usize::max(1, len.div_ceil(width));
            }
        }
        nested_y += y;
        (rect.x + x as u16 + 5, rect.y + nested_y as u16)
    }
    fn behaviour(
        &mut self,
        key: KeyEvent,
        focus: &mut Focus,
        cmd_line: &mut CmdLine,
        views: &mut Views,
        buffers: &mut Buffers,
        nodes: &mut Nodes,
    ) -> Result<(), EditorErr> {
        let cmd = key_to_cmd(key, views.get(*self));
        exec_cmd(cmd, *self, nodes, focus, cmd_line, views, buffers)?;
        enum Cmd {
            EnterVisual,
            EnterNormal,
            EnterInsert,
            Insert(char),
            NewLine,
            BackSpace,
            Undo,
            Redo,
            MoveUp,
            MoveDown,
            MoveRight,
            MoveLeft,
            MoveSelectionUp,
            MoveSelectionDown,
            MoveSelectionRight,
            MoveSelectionLeft,
            FocusUp,
            FocusDown,
            FocusRight,
            FocusLeft,
            EnterCmd,
            Yank,
            YankClipboard,
            Paste,
            PasteClipboard,
            Noop,
        }
        fn key_to_cmd(key: KeyEvent, view: &View) -> Cmd {
            match view.mode {
                Mode::Normal => match key.code {
                    KeyCode::Char('y') => Cmd::Yank,
                    KeyCode::Char('i') => Cmd::EnterInsert,
                    KeyCode::Char(':') => Cmd::EnterCmd,
                    KeyCode::Char('u') => Cmd::Undo,
                    KeyCode::Char('U') => Cmd::Redo,
                    KeyCode::Char('h') => Cmd::MoveLeft,
                    KeyCode::Char('j') => Cmd::MoveDown,
                    KeyCode::Char('k') => Cmd::MoveUp,
                    KeyCode::Char('l') => Cmd::MoveRight,
                    KeyCode::Char('H') => Cmd::FocusLeft,
                    KeyCode::Char('J') => Cmd::FocusDown,
                    KeyCode::Char('K') => Cmd::FocusUp,
                    KeyCode::Char('L') => Cmd::FocusRight,
                    KeyCode::Char('p') => Cmd::Paste,
                    KeyCode::Char('P') => Cmd::PasteClipboard,
                    KeyCode::Char('v') => Cmd::EnterVisual,
                    _ => Cmd::Noop,
                },
                Mode::Insert => match key.code {
                    KeyCode::Esc => Cmd::EnterNormal,
                    KeyCode::Backspace => Cmd::BackSpace,
                    KeyCode::Enter => Cmd::NewLine,
                    KeyCode::Char(c) => Cmd::Insert(c),
                    _ => Cmd::Noop,
                },
                Mode::Visual => match key.code {
                    KeyCode::Esc => Cmd::EnterNormal,
                    KeyCode::Char(':') => Cmd::EnterCmd,
                    KeyCode::Char('k') => Cmd::MoveSelectionUp,
                    KeyCode::Char('j') => Cmd::MoveSelectionDown,
                    KeyCode::Char('h') => Cmd::MoveSelectionLeft,
                    KeyCode::Char('l') => Cmd::MoveSelectionRight,
                    KeyCode::Char('y') => Cmd::Yank,
                    KeyCode::Char('Y') => Cmd::YankClipboard,
                    KeyCode::Char('p') => Cmd::Paste,
                    KeyCode::Char('P') => Cmd::PasteClipboard,
                    _ => Cmd::Noop,
                },
            }
        }
        fn exec_cmd(
            cmd: Cmd,
            vidx: ViewIdx,
            nodes: &mut Nodes,
            focus: &mut Focus,
            cmd_line: &mut CmdLine,
            views: &mut Views,
            buffers: &mut Buffers,
        ) -> Result<(), EditorErr> {
            fn enter_normal(view: &mut View, cmd_line: &mut CmdLine) {
                view.mode = Mode::Normal;
                queue!(stdout(), cursor::SetCursorStyle::SteadyBlock).unwrap();
                cmd_line.cursor = 0;
            }
            let (bidx, lidx) = {
                let mut curr = NodeIdx::Split(SplitIdx(0));
                let lidx = loop {
                    match curr {
                        NodeIdx::Split(s) => {
                            let Split {
                                children, focus: f, ..
                            } = nodes.splits.get(s.0).unwrap();
                            curr = *children.get(*f).unwrap();
                        }
                        NodeIdx::Leaf(l) => break l,
                    }
                };
                (views.get(vidx).buf, lidx)
            };
            match cmd {
                Cmd::EnterVisual => {
                    let v = views.get_mut(vidx);
                    v.mode = Mode::Visual;
                    v.selection = Some((v.cursor, v.cursor));
                    queue!(stdout(), cursor::SetCursorStyle::SteadyUnderScore)?;
                }
                Cmd::MoveSelectionUp => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    if line > 0 {
                        let line = line - 1;
                        let line_start = buffer.buf.line_to_char(line);
                        let line_len = buffer.buf.line(line).len_chars();
                        let col = view.prefered_x.min(line_len.saturating_sub(1));

                        view.cursor = line_start + col;
                        view.selection.as_mut().unwrap().1 =
                            usize::min(view.cursor + 1, buffer.buf.len_chars());
                    }
                    let buffer = buffers.get_mut(bidx);
                    View::scroll(view, &nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                }
                Cmd::MoveSelectionDown => {
                    let view = views.get_mut(vidx);
                    let buffer = buffers.get(bidx);
                    let len_lines = buffer.buf.len_lines();
                    let line = buffer.buf.char_to_line(view.cursor);
                    if line + 1 < len_lines {
                        let line = line + 1;
                        let start = buffer.buf.line_to_char(line);
                        let len = buffer.buf.line(line).len_chars();
                        let col = view.prefered_x.min(len.saturating_sub(1));
                        view.cursor = start + col;
                        view.selection.as_mut().unwrap().1 =
                            usize::min(view.cursor + 1, buffer.buf.len_chars());
                        View::scroll(
                            view,
                            &nodes.leaves.get(lidx.0).unwrap().rect,
                            buffers.get_mut(bidx),
                        );
                    }
                }
                Cmd::MoveSelectionRight => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let line_len = buffer.buf.line(line).len_chars();
                    if view.cursor < line_start + line_len.saturating_sub(1) {
                        let col = view.cursor - line_start;
                        let col = col + 1;
                        let col = col.min(buffer.buf.line(line).len_chars());
                        view.prefered_x = col;
                        view.cursor = line_start + col;
                        view.selection.as_mut().unwrap().1 =
                            usize::min(view.cursor + 1, buffer.buf.len_chars());
                    }
                }
                Cmd::MoveSelectionLeft => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let col = view.cursor - line_start;
                    if view.cursor > line_start {
                        let col = col - 1;
                        view.prefered_x = col;
                        view.cursor = line_start + col;
                        view.selection.as_mut().unwrap().1 =
                            usize::min(view.cursor + 1, buffer.buf.len_chars());
                    }
                }
                Cmd::Yank => {
                    let v = views.get_mut(vidx);
                    let b = buffers.get(v.buf);
                    let Some(selection) = &mut v.selection else {
                        return Ok(());
                    };
                    if selection.0 > selection.1 {
                        std::mem::swap(&mut selection.1, &mut selection.0);
                    }
                    v.clipboard = Some(b.buf.slice(selection.0..selection.1).to_string());
                    v.selection = None;
                    enter_normal(v, cmd_line);
                }
                Cmd::YankClipboard => {
                    let v = views.get_mut(vidx);
                    let b = buffers.get(v.buf);
                    let Some(selection) = &mut v.selection else {
                        return Ok(());
                    };
                    if selection.0 > selection.1 {
                        std::mem::swap(&mut selection.1, &mut selection.0);
                    }
                    yank_to_system_clipboard(&b.buf.slice(selection.0..selection.1).to_string())
                        .unwrap();
                    v.selection = None;
                    enter_normal(v, cmd_line);
                }
                Cmd::Paste => {
                    let v = views.get_mut(vidx);
                    let line = mem::take(&mut v.clipboard);
                    let Some(line) = line else { return Ok(()) };
                    let v = views.get_mut(vidx);
                    let b = buffers.get_mut(v.buf);
                    let c = usize::min(v.cursor + 1, b.buf.len_chars());
                    b.insert_string(v.off, c, &line);
                    b.undo.push(Edit::Insert {
                        idx: c,
                        text: line.clone(),
                    });
                    v.cursor += line.chars().count();
                    v.clipboard = Some(line);
                    v.selection = None;
                }

                Cmd::PasteClipboard => {
                    let s = match paste_to_system_clipboard() {
                        Ok(text) => text,
                        Err(_) => return Ok(()),
                    };
                    let v = views.get_mut(vidx);
                    let b = buffers.get_mut(v.buf);
                    let c = usize::min(v.cursor + 1, b.buf.len_chars());
                    b.insert_string(v.off, c, &s);
                    b.undo.push(Edit::Insert {
                        idx: c,
                        text: s.clone(),
                    });
                    v.cursor += s.chars().count();
                    v.selection = None;
                }
                Cmd::EnterCmd => {
                    cmd_line.enter_cmd_mode(vidx, focus, views, lidx);
                }
                Cmd::EnterInsert => {
                    queue!(stdout(), cursor::SetCursorStyle::SteadyBar)?;
                    views.get_mut(vidx).mode = Mode::Insert;
                }
                Cmd::EnterNormal => {
                    enter_normal(views.get_mut(vidx), cmd_line);
                }
                Cmd::FocusUp => {
                    nodes.focus_up(focus);
                }
                Cmd::FocusDown => {
                    nodes.focus_down(focus);
                }
                Cmd::FocusRight => {
                    nodes.focus_right(focus);
                }
                Cmd::FocusLeft => {
                    nodes.focus_left(focus);
                }
                Cmd::MoveUp => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    if line > 0 {
                        let line = line - 1;
                        let line_start = buffer.buf.line_to_char(line);
                        let line_len = buffer.buf.line(line).len_chars();
                        let col = view.prefered_x.min(line_len.saturating_sub(1));

                        view.cursor = line_start + col;
                        view.selection = None;
                    }
                    let buffer = buffers.get_mut(bidx);
                    View::scroll(view, &nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                }
                Cmd::MoveDown => {
                    let view = views.get_mut(vidx);
                    let buffer = buffers.get(bidx);
                    let len_lines = buffer.buf.len_lines();
                    let line = buffer.buf.char_to_line(view.cursor);
                    if line + 1 < len_lines {
                        let line = line + 1;
                        let start = buffer.buf.line_to_char(line);
                        let len = buffer.buf.line(line).len_chars();
                        let col = view.prefered_x.min(len.saturating_sub(1));
                        view.cursor = start + col;
                        view.selection = None;
                        View::scroll(
                            view,
                            &nodes.leaves.get(lidx.0).unwrap().rect,
                            buffers.get_mut(bidx),
                        );
                    }
                }
                Cmd::MoveRight => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let line_len = buffer.buf.line(line).len_chars();
                    if view.cursor < line_start + line_len.saturating_sub(1) {
                        let col = view.cursor - line_start;
                        let col = col + 1;
                        let col = col.min(buffer.buf.line(line).len_chars().saturating_sub(1));
                        view.prefered_x = col;
                        view.cursor = line_start + col;
                        view.selection = None;
                    }
                }
                Cmd::MoveLeft => {
                    let buffer = buffers.get(bidx);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let col = view.cursor - line_start;
                    if view.cursor > line_start {
                        let col = col - 1;
                        view.prefered_x = col;
                        view.cursor = line_start + col;
                        view.selection = None;
                    }
                }
                Cmd::Undo => {
                    let v = views.get_mut(vidx);
                    v.selection = None;
                    let buffer = buffers.get_mut(bidx);
                    if let Some(edit) = buffer.undo.pop() {
                        match edit {
                            Edit::Insert { idx, text } => {
                                buffer.redo.push(Edit::Delete {
                                    idx,
                                    text: text.clone(),
                                });
                                buffer.buf.remove(idx..idx + text.chars().count());
                                v.cursor = idx;
                            }
                            Edit::Delete { idx, text } => {
                                buffer.redo.push(Edit::Insert {
                                    idx,
                                    text: text.clone(),
                                });
                                buffer.buf.insert(idx, &text);
                                v.cursor = idx;
                            }
                        }
                        let line = buffer.buf.char_to_line(v.cursor);
                        let line_start = buffer.buf.line_to_char(line);
                        let col = v.cursor - line_start;
                        v.prefered_x = col;
                        return Ok(());
                    }
                    return Err(EditorErr::Msg("undo stack is empty".to_string()));
                }
                Cmd::Redo => {
                    let v = views.get_mut(vidx);
                    v.selection = None;
                    let buffer = buffers.get_mut(bidx);
                    if let Some(edit) = buffer.redo.pop() {
                        match edit {
                            Edit::Insert { idx, text } => {
                                buffer.buf.remove(idx..idx + text.chars().count());
                                v.cursor = idx;
                                buffer.undo.push(Edit::Delete { idx, text });
                            }
                            Edit::Delete { idx, text } => {
                                buffer.buf.insert(idx, &text);
                                v.cursor = idx;
                                buffer.undo.push(Edit::Insert { idx, text });
                            }
                        }
                        let line = buffer.buf.char_to_line(v.cursor);
                        let line_start = buffer.buf.line_to_char(line);
                        let col = v.cursor - line_start;
                        v.prefered_x = col;
                        let view = views.get_mut(vidx);
                        let buffer = buffers.get_mut(bidx);
                        View::scroll(view, &nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                        return Ok(());
                    }
                    return Err(EditorErr::Msg("redo stack is empty".to_string()));
                }
                Cmd::Insert(c) => {
                    let buffer = buffers.get_mut(bidx);
                    buffer.redo.clear();
                    views.get_mut(vidx).selection = None;
                    let v = views.get(vidx);
                    if let Some(edit) = buffer.undo.last_mut() {
                        match edit {
                            Edit::Insert {
                                idx: c_idx, text, ..
                            } => {
                                if *c_idx <= v.cursor && v.cursor <= *c_idx + text.chars().count() {
                                    let byte_idx = text
                                        .char_indices()
                                        .nth(v.cursor - *c_idx)
                                        .map(|(b_idx, _)| b_idx)
                                        .unwrap_or(text.len());
                                    text.insert_str(byte_idx, &c.to_string());
                                } else {
                                    buffer.undo.push(Edit::Insert {
                                        idx: v.cursor,
                                        text: c.into(),
                                    });
                                }
                            }
                            Edit::Delete { .. } => {
                                buffer.undo.push(Edit::Insert {
                                    idx: v.cursor,
                                    text: c.into(),
                                });
                            }
                        }
                    } else {
                        buffer.undo.push(Edit::Insert {
                            idx: v.cursor,
                            text: c.into(),
                        });
                    }
                    buffer.insert(v.off, v.cursor, c);
                    let view = views.get_mut(vidx);
                    let line = buffer.buf.char_to_line(view.cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let col = view.cursor + 1 - line_start;

                    let line_end = buffer.buf.line(line).len_chars();
                    let col = col.min(line_end.saturating_sub(1));

                    view.cursor = line_start + col;
                    view.prefered_x = view.cursor - line_start;
                }
                Cmd::NewLine => {
                    let v = views.get_mut(vidx);
                    v.selection = None;
                    let buffer = buffers.get_mut(bidx);
                    buffer.redo.clear();
                    buffer.insert(v.off, v.cursor, '\n');
                    if let Some(edit) = buffer.undo.last_mut() {
                        match edit {
                            Edit::Insert {
                                idx: c_idx, text, ..
                            } => {
                                if *c_idx <= v.cursor && v.cursor <= *c_idx + text.chars().count() {
                                    let byte_idx = text
                                        .char_indices()
                                        .nth(v.cursor - *c_idx)
                                        .map(|(b_idx, _)| b_idx)
                                        .unwrap_or(text.len());
                                    text.insert_str(byte_idx, &'\n'.to_string());
                                } else {
                                    buffer.undo.push(Edit::Insert {
                                        idx: v.cursor,
                                        text: '\n'.into(),
                                    });
                                }
                            }
                            Edit::Delete { .. } => {
                                buffer.undo.push(Edit::Insert {
                                    idx: v.cursor,
                                    text: '\n'.into(),
                                });
                            }
                        }
                    } else {
                        buffer.undo.push(Edit::Insert {
                            idx: v.cursor,
                            text: '\n'.into(),
                        });
                    }
                    let line = buffer.buf.char_to_line(v.cursor) + 1;
                    let len_lines = buffer.buf.len_lines();
                    let line = line.min(len_lines);
                    let line_start = buffer.buf.line_to_char(line);
                    v.cursor = line_start;
                    View::scroll(v, &nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                }
                Cmd::BackSpace => {
                    let v = views.get_mut(vidx);
                    v.selection = None;
                    let buffer = buffers.get_mut(bidx);
                    buffer.redo.clear();
                    if v.cursor != 0 {
                        let del = buffer.buf.slice(v.cursor - 1..v.cursor).to_string();
                        if let Some(edit) = buffer.undo.last_mut() {
                            match edit {
                                Edit::Insert { .. } => {
                                    buffer.undo.push(Edit::Delete {
                                        idx: v.cursor - 1,
                                        text: del,
                                    });
                                }
                                Edit::Delete {
                                    idx: xidx, text, ..
                                } => {
                                    if *xidx == v.cursor {
                                        *xidx -= 1;
                                        text.insert_str(0, &del);
                                    } else {
                                        buffer.undo.push(Edit::Delete {
                                            idx: v.cursor - 1,
                                            text: del,
                                        });
                                    }
                                }
                            }
                        } else {
                            buffer.undo.push(Edit::Delete {
                                idx: v.cursor - 1,
                                text: del,
                            });
                        }
                        let line = buffer.buf.char_to_line(v.cursor);
                        let line_start = buffer.buf.line_to_char(line);
                        let col = v.cursor - line_start;
                        let prev_start = buffer.buf.line_to_char(line.saturating_sub(1));
                        let prev_len = buffer
                            .buf
                            .line(line.saturating_sub(1))
                            .len_chars()
                            .saturating_sub(1);
                        buffer.buf.remove(v.cursor - 1..v.cursor);
                        if v.cursor > line_start {
                            let col = col - 1;
                            v.prefered_x = col;
                            v.cursor = line_start + col;
                        } else {
                            v.prefered_x = prev_len;
                            v.cursor = prev_start + prev_len;
                        }
                        View::scroll(v, &nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                    }
                }
                Cmd::Noop => {}
            }
            Ok(())
        }
        Ok(())
    }
}

struct BufferList {}

fn sketch_border1(rect: &Rect, screen: &mut ScreenBuffer) -> Rect {
    sketch_border(rect, screen, '┌', '┐', '└', '┘', '─', '│', FG, BG)
}
fn sketch_border(
    rect: &Rect,
    screen: &mut ScreenBuffer,
    uppper_left: char,
    upper_right: char,
    bottom_left: char,
    bottom_right: char,
    horizontal: char,
    vertical: char,
    fg: Color,
    bg: Color,
) -> Rect {
    let mut r = *rect;
    screen.set_cell_xy(
        r.x,
        r.y,
        Cell {
            c: uppper_left,
            fg,
            bg,
        },
    );
    screen.set_cell_xy(
        r.x + r.width,
        r.y,
        Cell {
            c: upper_right,
            fg,
            bg,
        },
    );
    screen.set_cell_xy(
        r.x,
        r.y + r.height,
        Cell {
            c: bottom_left,
            fg,
            bg,
        },
    );
    screen.set_cell_xy(
        r.x + r.width,
        r.y + r.height,
        Cell {
            c: bottom_right,
            fg,
            bg,
        },
    );
    screen.set_string_xy(
        r.x + 1,
        r.y,
        &horizontal.to_string().repeat((r.width - 1) as usize),
        fg,
        bg,
    );
    screen.set_string_xy(
        r.x + 1,
        r.y + r.height,
        &horizontal.to_string().repeat((r.width - 1) as usize),
        fg,
        bg,
    );
    for y in 1..r.height {
        screen.set_cell_xy(
            r.x,
            y + r.y,
            Cell {
                c: vertical,
                fg,
                bg,
            },
        );
        screen.set_cell_xy(
            r.x + r.width,
            y + r.y,
            Cell {
                c: vertical,
                fg,
                bg,
            },
        );
    }
    r.x += 1;
    r.y += 1;
    r.width = r.width.saturating_sub(2);
    r.height = r.height.saturating_sub(2);
    r
}

impl Component for BufferList {
    fn sketch(&self, r: &Rect, _views: &Views, buffers: &Buffers, screen: &mut ScreenBuffer) {
        let r = sketch_border1(r, screen);
        let dirty = if buffers.data.get(0).unwrap().undo.is_empty() {
            ""
        } else {
            "Dirty"
        };

        let s = format!("{} {} {}", 0, "SCRATCH", dirty);
        let s = format!("{:<width$}", s, width = r.width as usize + 1);

        screen.set_string_xy(r.x, r.y, &s, FG, BG);

        let empty = &" ".repeat((r.width + 1) as usize);
        for y in r.y..r.y + r.height {
            if y as usize > buffers.data.len() - 1 {
                screen.set_string_xy(r.x, y + 1, empty, FG, BG);
                continue;
            }

            let dirty = if buffers.data.get(y as usize).unwrap().undo.is_empty() {
                ""
            } else {
                "Dirty"
            };

            let file_path = match buffers.data.get(y as usize) {
                None => "NEW_FILE".to_string(),
                Some(b) => match &b.file {
                    Some(path) => path.to_string_lossy().to_string(),
                    None => "NEW_FILE".to_string(),
                },
            };

            let s = format!("{} {} {}", y, file_path, dirty);
            let s = format!("{:<width$}", s, width = (r.width + 1) as usize);
            screen.set_string_xy(r.x, y + 1, &s, FG, BG);
        }
    }
    fn cursor_xy(&self, rect: &Rect, _views: &Views, _buffers: &Buffers) -> (u16, u16) {
        (rect.x, rect.y)
    }
    fn behaviour(
        &mut self,
        key: KeyEvent,
        focus: &mut Focus,
        _cmd_line: &mut CmdLine,
        views: &mut Views,
        _buffers: &mut Buffers,
        nodes: &mut Nodes,
    ) -> Result<(), EditorErr> {
        match key.code {
            KeyCode::Esc => {
                let (l, lidx) = {
                    let Focus::Leaf(lidx) = focus else { panic!() };
                    (nodes.leaves.get(lidx.0).unwrap(), lidx.clone())
                };
                nodes.remove_child(l.parent, views, focus, NodeIdx::Leaf(lidx));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
struct LeafIdx(usize);

struct Leaf {
    parent: SplitIdx,
    rect: Rect,
    comp: Box<dyn Component>,
}

#[derive(Clone, Copy, PartialEq)]
enum NodeIdx {
    Leaf(LeafIdx),
    Split(SplitIdx),
}

#[derive(Clone, Copy, PartialEq)]
struct SplitIdx(usize);

struct Split {
    parent: Option<SplitIdx>,
    children: Vec<NodeIdx>,
    direction: Direction,
    focus: usize,
    rect: Rect,
}

#[derive(Clone, PartialEq, Eq)]
enum Direction {
    Horizontal,
    Vertical,
}

enum Focus {
    Leaf(LeafIdx),
    CmdLine,
}

//index into nodes.roots
const ROOT_OVERLAY: usize = 1;
const ROOT_TEXT_VIEW: usize = 0;
struct Nodes {
    roots: Vec<SplitIdx>,
    splits: Vec<Split>,
    leaves: Vec<Leaf>,
    free_splits: Vec<usize>,
    free_leaves: Vec<usize>,
}

impl Nodes {
    fn new() -> Self {
        Nodes {
            roots: vec![],
            splits: vec![],
            leaves: vec![],
            free_splits: vec![],
            free_leaves: vec![],
        }
    }

    fn push_leaf(&mut self, leaf: Leaf) -> LeafIdx {
        if self.free_leaves.is_empty() {
            let lidx = self.leaves.len();
            self.leaves.push(leaf);
            LeafIdx(lidx)
        } else {
            let lidx = self.free_leaves.pop().unwrap();
            self.leaves[lidx] = leaf;
            LeafIdx(lidx)
        }
    }
    fn push_branch(&mut self, split: Split) -> SplitIdx {
        if self.free_splits.is_empty() {
            let sidx = self.splits.len();
            self.splits.push(split);
            SplitIdx(sidx)
        } else {
            let sidx = self.free_splits.pop().unwrap();
            self.splits[sidx] = split;
            SplitIdx(sidx)
        }
    }

    fn new_root(&mut self, rect: Rect, direction: Direction) -> SplitIdx {
        let new_root = self.push_branch(Split {
            parent: None,
            children: vec![],
            focus: 0,
            direction,
            rect,
        });
        self.roots.push(new_root);
        new_root
    }
    fn new_split(
        &mut self,
        vidx: ViewIdx,
        parent: SplitIdx,
        direction: Direction,
        constraint: Option<Constraints>,
    ) -> (LeafIdx, SplitIdx) {
        let constraint: Constraints = {
            if let Some(c) = constraint {
                c
            } else {
                Constraints {
                    min_height: None,
                    max_height: None,
                    min_width: None,
                    max_width: None,
                }
            }
        };
        let new_parent = self.push_branch(Split {
            parent: Some(parent),
            children: vec![],
            focus: 0,
            direction,
            rect: Rect {
                x: 0,
                y: 0,
                height: 0,
                width: 0,
                constraint,
            },
        });
        self.splits[parent.0]
            .children
            .push(NodeIdx::Split(new_parent));
        let comp: Box<dyn Component> = Box::new(vidx);
        let lidx = self.new_leaf(comp, new_parent, None);
        self.recalc(parent);
        (lidx, new_parent)
    }
    fn new_leaf(
        &mut self,
        comp: Box<dyn Component>,
        parent: SplitIdx,
        constraint: Option<Constraints>,
    ) -> LeafIdx {
        let constraint = {
            if let Some(c) = constraint {
                c
            } else {
                Constraints {
                    min_width: None,
                    min_height: None,
                    max_width: None,
                    max_height: None,
                }
            }
        };
        let lidx = self.push_leaf(Leaf {
            parent,
            comp,
            rect: Rect {
                x: 0,
                y: 0,
                height: 0,
                width: 0,
                constraint,
            },
        });
        self.splits[parent.0].children.push(NodeIdx::Leaf(lidx));
        self.recalc(parent);
        lidx
    }

    fn remove_child(
        &mut self,
        parent: SplitIdx,
        views: &mut Views,
        focus: &mut Focus,
        child: NodeIdx,
    ) {
        let Split {
            children, focus: f, ..
        } = &mut self.splits[parent.0];
        match child {
            NodeIdx::Leaf(lidx) => {
                children.retain(|x| match x {
                    NodeIdx::Leaf(l) => l.0 != lidx.0,
                    _ => true,
                });
            }
            NodeIdx::Split(sidx) => {
                children.retain(|x| match x {
                    NodeIdx::Split(s) => s.0 != sidx.0,
                    _ => true,
                });
            }
        }
        if children.is_empty() {
            *f = 0;
            self.reflow(focus, views, parent);
            let parent = {
                let Focus::Leaf(l) = focus else {
                    panic!();
                };
                let Leaf { parent, .. } = self.leaves.get(l.0).unwrap();
                parent
            };
            self.recalc(*parent);
        } else {
            *f = (*f + children.len() - 1) % children.len();
            self.recalc(parent);
        }
        self.remove(child);
        let mut curr = NodeIdx::Split(SplitIdx(0));
        let lidx = loop {
            match curr {
                NodeIdx::Split(s) => {
                    let Split {
                        children, focus: f, ..
                    } = self.splits.get(s.0).unwrap();
                    curr = *children.get(*f).unwrap();
                }
                NodeIdx::Leaf(l) => break l,
            }
        };
        *focus = Focus::Leaf(lidx);
    }
    fn remove(&mut self, nidx: NodeIdx) {
        match nidx {
            NodeIdx::Leaf(lidx) => {
                self.free_leaves.push(lidx.0);
            }
            NodeIdx::Split(sidx) => {
                self.free_splits.push(sidx.0);
            }
        }
    }

    fn recalc(&mut self, sidx: SplitIdx) {
        let curr = sidx;
        let Split {
            children,
            direction,
            rect,
            ..
        } = self.splits.get(curr.0).unwrap();
        if children.is_empty() {
            return;
        }
        let resize: Vec<(u16, NodeIdx)> = {
            let (mut size_left, mut remainder) = {
                match direction {
                    Direction::Vertical => (rect.width, rect.width % children.len() as u16),
                    Direction::Horizontal => (rect.height, rect.height % children.len() as u16),
                }
            };
            let mut resize: Vec<(u16, NodeIdx)> = vec![]; //main axis either width or height
            for n in children.iter() {
                match n {
                    NodeIdx::Leaf(l) => {
                        let l = self.leaves.get(l.0).unwrap();
                        let mut min = 0;
                        match direction {
                            Direction::Horizontal => {
                                if let Some(h) = l.rect.constraint.min_height {
                                    min = h;
                                    size_left -= h;
                                }
                            }
                            Direction::Vertical => {
                                if let Some(w) = l.rect.constraint.min_width {
                                    min = w;
                                    size_left -= w;
                                }
                            }
                        }
                        resize.push((min, *n));
                    }
                    NodeIdx::Split(s) => {
                        let s = self.splits.get(s.0).unwrap();
                        let mut min = 0;
                        match direction {
                            Direction::Horizontal => {
                                if let Some(h) = s.rect.constraint.min_height {
                                    min = h;
                                    size_left -= h;
                                }
                            }
                            Direction::Vertical => {
                                if let Some(w) = s.rect.constraint.min_width {
                                    min = w;
                                    size_left -= w;
                                }
                            }
                        }
                        resize.push((min, *n));
                    }
                }
            }

            let mut non_maxed: Vec<usize> = (0..resize.len()).collect();
            while !non_maxed.is_empty() && size_left != 0 {
                let width_per_child = size_left / non_maxed.len() as u16;
                size_left = 0;
                let mut i = 0;
                while i < non_maxed.len() {
                    let idx = non_maxed[i];
                    let (s, n) = &mut resize[idx];
                    let max = {
                        match direction {
                            Direction::Vertical => match n {
                                NodeIdx::Leaf(l) => {
                                    self.leaves.get(l.0).unwrap().rect.constraint.max_width
                                }
                                NodeIdx::Split(s) => {
                                    self.splits.get(s.0).unwrap().rect.constraint.max_width
                                }
                            },
                            Direction::Horizontal => match n {
                                NodeIdx::Leaf(l) => {
                                    self.leaves.get(l.0).unwrap().rect.constraint.max_height
                                }
                                NodeIdx::Split(s) => {
                                    self.leaves.get(s.0).unwrap().rect.constraint.max_height
                                }
                            },
                        }
                    };
                    *s += width_per_child;
                    if remainder > 0 {
                        *s += 1;
                        remainder -= 1;
                    }
                    if let Some(max) = max {
                        if *s >= max {
                            size_left += s.saturating_sub(max);
                            *s = max;
                            non_maxed.swap_remove(i);
                            continue;
                        }
                    }
                    i += 1;
                }
            }
            resize
        };
        let (mut x, mut y) = (rect.x, rect.y);
        let direction = direction.clone();
        let rect = rect.clone();
        for (len, n) in resize {
            match direction {
                Direction::Vertical => match n {
                    NodeIdx::Leaf(l) => {
                        let l = self.leaves.get_mut(l.0).unwrap();
                        l.rect.x = x;
                        l.rect.y = y;
                        l.rect.width = len;
                        x += l.rect.width;
                        l.rect.height = l.rect.constraint.max_height.unwrap_or(rect.height);
                    }
                    NodeIdx::Split(sidx) => {
                        let s = self.splits.get_mut(sidx.0).unwrap();
                        s.rect.x = x;
                        s.rect.y = y;
                        s.rect.width = len;
                        x += s.rect.width;
                        s.rect.height = s.rect.constraint.max_height.unwrap_or(rect.height);
                        self.recalc(sidx);
                    }
                },
                Direction::Horizontal => match n {
                    NodeIdx::Leaf(l) => {
                        let l = self.leaves.get_mut(l.0).unwrap();
                        l.rect.x = x;
                        l.rect.y = y;
                        l.rect.height = len;
                        y += l.rect.height;
                        l.rect.width = l.rect.constraint.max_width.unwrap_or(rect.width);
                    }
                    NodeIdx::Split(sidx) => {
                        let s = self.splits.get_mut(sidx.0).unwrap();
                        s.rect.x = x;
                        s.rect.y = y;
                        s.rect.height = len;
                        y += s.rect.height;
                        s.rect.width = s.rect.constraint.max_width.unwrap_or(rect.width);
                        self.recalc(sidx);
                    }
                },
            }
        }
    }

    fn reflow(&mut self, focus: &mut Focus, views: &mut Views, parent: SplitIdx) {
        let mut to_remove: Option<(SplitIdx, usize, NodeIdx)> = None; //parent, child, node
        let mut curr = parent;
        loop {
            let Split {
                parent, children, ..
            } = self.splits.get(curr.0).unwrap();
            if children.is_empty() {
                if let Some(p) = parent {
                    let Split { children, .. } = self.splits.get(p.0).unwrap();
                    to_remove = Some((
                        *p,
                        children
                            .iter()
                            .position(|x| *x == NodeIdx::Split(curr))
                            .unwrap(),
                        NodeIdx::Split(curr),
                    ));
                    curr = *p
                }
            }
            match to_remove {
                Some(s) => {
                    let Split {
                        children, focus, ..
                    } = &mut self.splits[s.0.0];
                    children.remove(s.1);
                    *focus = focus.saturating_sub(1);
                    self.remove(s.2);
                    to_remove = None;
                }
                None => break,
            }
        }

        //root cannot be empty
        let Split {
            children, focus: f, ..
        } = &mut self.splits[self.roots[ROOT_TEXT_VIEW].0];
        if children.is_empty() {
            let vidx = views.push(View::new(SCRATCH));
            let comp: Box<dyn Component> = Box::new(vidx);
            *f = 0;
            self.new_leaf(comp, self.roots[ROOT_TEXT_VIEW], None);
        }

        let mut curr = NodeIdx::Split(self.roots[ROOT_TEXT_VIEW]);
        while let NodeIdx::Split(s) = curr {
            let Split {
                children, focus: f, ..
            } = &self.splits[s.0];
            curr = *children.get(*f).unwrap();
        }
        let curr = {
            match curr {
                NodeIdx::Leaf(l) => l,
                _ => panic!(),
            }
        };
        *focus = Focus::Leaf(curr)
    }

    fn paint(
        &self,
        focus: &Focus,
        cmd_line: &CmdLine,
        views: &Views,
        buffers: &Buffers,
        old: &mut ScreenBuffer,
        new: &mut ScreenBuffer,
    ) -> io::Result<()> {
        for r in &self.roots {
            sketch(&self, NodeIdx::Split(*r), views, buffers, old, new);
        }
        cmd_line.sketch(focus, new);
        new.print(old)?;
        match focus {
            Focus::CmdLine => {
                queue!(
                    stdout(),
                    MoveTo(cmd_line.cursor as u16 + 1, cmd_line.rect.y)
                )?;
            }
            Focus::Leaf(l) => {
                let Leaf { comp, rect, .. } = self.leaves.get(l.0).unwrap();
                let (x, y) = comp.cursor_xy(rect, views, buffers).clone();
                queue!(stdout(), MoveTo(x, y))?;
            }
            _ => {}
        }
        fn sketch(
            nodes: &Nodes,
            nidx: NodeIdx,
            views: &Views,
            buffers: &Buffers,
            old: &mut ScreenBuffer,
            new: &mut ScreenBuffer,
        ) {
            match nidx {
                NodeIdx::Split(s) => {
                    let s = &nodes.splits[s.0];
                    for (i, n) in s.children.iter().enumerate() {
                        if i != s.focus {
                            sketch(nodes, *n, views, buffers, old, new);
                        }
                    }
                    if let Some(nidx) = s.children.get(s.focus) {
                        sketch(nodes, *nidx, views, buffers, old, new);
                    }
                }
                NodeIdx::Leaf(l) => {
                    let l = &nodes.leaves[l.0];
                    l.comp.sketch(&l.rect, views, buffers, new);
                }
            }
        }
        Ok(())
    }

    fn focus_right(&mut self, focus: &mut Focus) {
        let l = {
            let Focus::Leaf(l) = focus else { panic!() };
            l
        };
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let x = rect.x + rect.width;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.x >= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.x >= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break Focus::Leaf(l);
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    fn focus_left(&mut self, focus: &mut Focus) {
        let l = {
            let Focus::Leaf(l) = focus else { panic!() };
            l
        };
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let x = rect.x;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate().rev() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.x + rect.width <= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.x + rect.width <= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break Focus::Leaf(l);
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    fn focus_up(&mut self, focus: &mut Focus) {
        let l = {
            let Focus::Leaf(l) = focus else { panic!() };
            l
        };
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let y = rect.y;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate().rev() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.y + rect.height <= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.y + rect.height <= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break Focus::Leaf(l);
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    fn focus_down(&mut self, focus: &mut Focus) {
        let l = {
            let Focus::Leaf(l) = focus else { panic!() };
            l
        };
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let y = rect.y + rect.height;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.y >= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.y >= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break Focus::Leaf(l);
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    c: char,
    fg: Color,
    bg: Color,
}
struct ScreenBuffer {
    cells: Vec<Cell>,
    width: u16,
    height: u16,
}
impl ScreenBuffer {
    fn set_cell_xy(&mut self, x: u16, y: u16, cell: Cell) {
        let idx = y * self.width + x;
        self.cells[idx as usize] = cell;
    }
    fn set_string_xy(&mut self, x: u16, y: u16, s: &str, fg: Color, bg: Color) {
        for (i, c) in s.chars().enumerate() {
            let xx = x + i as u16;
            if xx >= self.width || y >= self.height {
                break;
            }
            self.set_cell_xy(xx, y, Cell { c, fg, bg });
        }
    }
    fn clear_buffer(&mut self) {
        self.cells.fill(Cell {
            c: ' ',
            fg: FG,
            bg: BG,
        });
    }
    fn print(&mut self, prev: &mut ScreenBuffer) -> io::Result<()> {
        let mut out = stdout().lock();
        let mut current_fg = None;
        let mut current_bg = None;

        for y in 0..self.height {
            let mut x = 0;
            while x < self.width {
                let idx = (y * self.width + x) as usize;
                let old = prev.cells[idx];
                let new = self.cells[idx];
                if new == old {
                    x += 1;
                    continue;
                }
                let start_x = x;
                let style_fg = new.fg;
                let style_bg = new.bg;
                let mut line = String::new();
                while x < self.width {
                    let idx = (y * self.width + x) as usize;
                    let old = prev.cells[idx];
                    let new = self.cells[idx];
                    //stop if unchganged
                    if new == old {
                        break;
                    }
                    // stop if style changes
                    if new.fg != style_fg || new.bg != style_bg {
                        break;
                    }
                    line.push(new.c);
                    x += 1;
                }
                queue!(out, MoveTo(start_x, y))?;
                if current_fg != Some(style_fg) {
                    queue!(out, SetForegroundColor(style_fg))?;
                    current_fg = Some(style_fg);
                }
                if current_bg != Some(style_bg) {
                    queue!(out, SetBackgroundColor(style_bg))?;
                    current_bg = Some(style_bg);
                }
                queue!(out, Print(line))?;
            }
        }

        queue!(out, ResetColor)?;

        std::mem::swap(self, prev);
        self.clear_buffer();

        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Insert,
    Visual,
}

fn key_to_exec(
    key: KeyEvent,
    nodes: &mut Nodes,
    focus: &mut Focus,
    cmd_line: &mut CmdLine,
    views: &mut Views,
    buffers: &mut Buffers,
) -> Result<(), EditorErr> {
    match focus {
        Focus::CmdLine => {
            let cmd = match key.code {
                KeyCode::Char(c) => Cmd::Insert(c),
                KeyCode::Esc => Cmd::EnterNormal,
                KeyCode::Backspace => Cmd::BackSpace,
                KeyCode::Left => Cmd::MoveLeft,
                KeyCode::Right => Cmd::MoveRight,
                KeyCode::Enter => Cmd::Exec,
                _ => Cmd::Noop,
            };
            exec_cmd(cmd, cmd_line, nodes, focus, views, buffers)?;
            enum Cmd {
                BufferList,
                EnterNormal,
                Exec,
                Insert(char),
                BackSpace,
                MoveLeft,
                MoveRight,
                Quit(bool),
                Save(Option<String>),
                Open(Option<String>),
                SwitchBuffer(BufferIdx),
                Close(Option<BufferIdx>, bool),
                Split,
                SplitV,
                SplitH,
                ViewClose,
                Noop,
            }
            fn parse_cmd(s: String) -> Result<Cmd, String> {
                fn parse_args(s: &str) -> Vec<String> {
                    let mut args = Vec::new();
                    let mut current = String::new();
                    let mut in_quotes = false;

                    for c in s.chars() {
                        match c {
                            '"' => in_quotes = !in_quotes,
                            ' ' if !in_quotes => {
                                if !current.is_empty() {
                                    args.push(current.clone());
                                    current.clear();
                                }
                            }
                            _ => current.push(c),
                        }
                    }
                    if !current.is_empty() {
                        args.push(current);
                    }
                    args
                }
                let s = s.trim();
                let mut parts = s.splitn(2, ' ');
                let cmd = parts.next().ok_or(format!("unknown command: {}", s))?;
                let rest = parts.next().unwrap_or("");
                match cmd {
                    "bufferlist" | "bl" => Ok(Cmd::BufferList),
                    "q" => Ok(Cmd::Quit(false)),
                    "Q" => Ok(Cmd::Quit(true)),
                    "w" => {
                        let args = parse_args(rest);
                        Ok(Cmd::Save(args.get(0).cloned()))
                    }
                    "open" | "o" => {
                        let args = parse_args(rest);
                        if let Some(arg) = args.get(0) {
                            if let Ok(idx) = arg.parse::<usize>() {
                                Ok(Cmd::SwitchBuffer(BufferIdx { idx }))
                            } else {
                                Ok(Cmd::Open(Some(arg.clone())))
                            }
                        } else {
                            Ok(Cmd::Open(None))
                        }
                    }
                    "split" | "s" => Ok(Cmd::Split),
                    "splitv" | "sv" => Ok(Cmd::SplitV),
                    "splith" | "sh" => Ok(Cmd::SplitH),
                    "close" | "c" => {
                        let mut args = Vec::new();
                        args.push(rest);
                        if let Some(arg) = args.get(0) {
                            if let Ok(idx) = arg.parse::<usize>() {
                                Ok(Cmd::Close(Some(BufferIdx { idx }), false))
                            } else {
                                Ok(Cmd::Close(None, false))
                            }
                        } else {
                            Ok(Cmd::Close(None, false))
                        }
                    }
                    "CLOSE" | "C" => {
                        let mut args = Vec::new();
                        args.push(rest);
                        if let Some(arg) = args.get(0) {
                            if let Ok(idx) = arg.parse::<usize>() {
                                Ok(Cmd::Close(Some(BufferIdx { idx }), true))
                            } else {
                                Ok(Cmd::Close(None, true))
                            }
                        } else {
                            Ok(Cmd::Close(None, true))
                        }
                    }
                    "viewclose" | "vc" => Ok(Cmd::ViewClose),
                    _ => Err(format!("unknown command: {}", cmd)),
                }
            }
            fn exec_cmd(
                cmd: Cmd,
                cmd_line: &mut CmdLine,
                nodes: &mut Nodes,
                focus: &mut Focus,
                views: &mut Views,
                buffers: &mut Buffers,
            ) -> Result<(), EditorErr> {
                let (bidx, vidx, lidx, parent) = {
                    let l = nodes
                        .leaves
                        .get(cmd_line.last_view.0.0)
                        .expect(&format!("lidx invalid:{}", cmd_line.last_view.0.0));
                    (
                        views.get(cmd_line.last_view.1).buf,
                        cmd_line.last_view.1,
                        cmd_line.last_view.0,
                        l.parent,
                    )
                };
                fn enter_normal(focus: &mut Focus, lidx: LeafIdx, cmd_line: &mut CmdLine) {
                    queue!(stdout(), cursor::SetCursorStyle::SteadyBlock).unwrap();
                    cmd_line.cursor = 0;
                    *focus = Focus::Leaf(lidx);
                }
                match cmd {
                    Cmd::BufferList => {
                        let comp: Box<dyn Component> = Box::new(BufferList {});
                        *focus = Focus::Leaf(nodes.new_leaf(
                            comp,
                            nodes.roots[ROOT_OVERLAY],
                            Some(Constraints {
                                max_width: Some(20),
                                max_height: Some(20),
                                min_height: None,
                                min_width: None,
                            }),
                        ));
                        queue!(stdout(), cursor::SetCursorStyle::SteadyBlock)?;
                    }
                    Cmd::Exec => match parse_cmd(cmd_line.input.clone()) {
                        Ok(cmd) => {
                            exec_cmd(cmd, cmd_line, nodes, focus, views, buffers)?;
                        }
                        Err(s) => return Err(EditorErr::Msg(s)),
                    },
                    Cmd::EnterNormal => {
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::Insert(c) => {
                        cmd_line.insert(c);
                    }
                    Cmd::BackSpace => {
                        cmd_line.backspace();
                    }
                    Cmd::MoveLeft => {
                        cmd_line.cursor = cmd_line.cursor.saturating_sub(1);
                    }
                    Cmd::MoveRight => {
                        cmd_line.cursor = cmd_line.cursor.saturating_add(1);
                    }
                    Cmd::Open(file) => {
                        let view = views.get_mut(vidx);
                        view.off = 0;
                        view.cursor = 0;
                        view.prefered_x = 0;
                        let buffer = if let Some(f) = file {
                            if let Some(b) = buffers.get_by_path(&f) {
                                let buffer = buffers.get(*b);
                                let line = buffer.buf.char_to_line(buffer.last_cursor);
                                let line_start = buffer.buf.line_to_char(line);
                                let col = buffer.last_cursor - line_start;
                                view.cursor = buffer.last_cursor;
                                view.prefered_x = col;
                                view.off = buffer.last_off;
                                *b
                            } else {
                                buffers.push(Buffer::new(Some(&f), 0)?)
                            }
                        } else {
                            buffers.push(Buffer::new(None, 0)?)
                        };
                        view.buf = buffer;
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::Close(bidx, force) => {
                        let view = views.get_mut(vidx);
                        let mut bidx = { if let Some(idx) = bidx { idx } else { view.buf } };
                        let curr_buffer = buffers.get(bidx);
                        if bidx != SCRATCH {
                            if curr_buffer.check_flag(Buffer::READ_ONLY) {
                                return Err(EditorErr::ReadOnly(bidx));
                            }
                            if !curr_buffer.undo.is_empty() && force == false {
                                return Err(EditorErr::Dirty(bidx));
                            } else {
                                if view.buf == bidx {
                                    view.buf = SCRATCH;
                                    cmd_line.input.clear();
                                    view.off = 0;
                                    view.cursor = 0;
                                    view.prefered_x = 0;
                                }
                                buffers.remove(&mut bidx);
                            }
                        } else {
                            return Err(EditorErr::Msg("will not close special buffer: 0".into()));
                        }
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::ViewClose => {
                        nodes.remove_child(parent, views, focus, NodeIdx::Leaf(lidx));
                        let mut curr = NodeIdx::Split(*nodes.roots.get(ROOT_TEXT_VIEW).unwrap());
                        let lidx = loop {
                            match curr {
                                NodeIdx::Split(s) => {
                                    let Split {
                                        children, focus: f, ..
                                    } = nodes.splits.get(s.0).unwrap();
                                    curr = *children.get(*f).unwrap();
                                }
                                NodeIdx::Leaf(l) => break l,
                            }
                        };
                        *focus = Focus::Leaf(lidx);
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::Save(f) => {
                        let buffer = buffers.get_mut(bidx);
                        if buffer.check_flag(Buffer::READ_ONLY) {
                            return Err(EditorErr::ReadOnly(bidx));
                        }
                        if buffer.check_flag(Buffer::SCRATCH) {
                            return Err(EditorErr::Msg(format!(
                                "cant save, buffer: {} is scratch",
                                bidx.idx
                            )));
                        }
                        if let Some(new) = f {
                            buffer.save(Some(new))?;
                            buffer.undo.clear();
                            buffer.redo.clear();
                        } else {
                            if let Some(_) = &buffer.file {
                                match buffer.save(None) {
                                    Err(error) => return Err(EditorErr::Io(error)),
                                    Ok(_) => {
                                        buffer.undo.clear();
                                        buffer.redo.clear();
                                    }
                                }
                            } else {
                                return Err(EditorErr::Msg("new file needs name".into()));
                            }
                        }
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::SwitchBuffer(idx) => {
                        if idx.idx < buffers.len() {
                            if buffers.get(idx).check_flag(Buffer::NON_NAVIGATABLE) {
                                return Err(EditorErr::Msg(format!(
                                    "buffer {} is non navigatable",
                                    idx.idx
                                )))?;
                            }
                            let view = views.get_mut(vidx);
                            let buffer = buffers.get_mut(view.buf);
                            buffer.last_off = view.off;
                            buffer.last_cursor = view.cursor;
                            let buffer = buffers.get_mut(idx);
                            if buffer.buf.len_chars() == 0 {
                                if let Some(p) = &buffer.file {
                                    let file = File::open(p)?;
                                    let reader = BufReader::new(file);
                                    buffer.buf = Rope::from_reader(reader)?;
                                }
                            }
                            view.buf = idx;
                            view.cursor = buffer.last_cursor;
                            view.off = buffer.last_off;
                            let line = buffer.buf.char_to_line(buffer.last_cursor);
                            let line_start = buffer.buf.line_to_char(line);
                            let col = buffer.last_cursor - line_start;
                            view.cursor = buffer.last_cursor;
                            view.prefered_x = col;
                            view.scroll(&nodes.leaves.get(lidx.0).unwrap().rect, buffer);
                            enter_normal(focus, lidx, cmd_line);
                        } else {
                            return Err(EditorErr::InvalidBuffer);
                        }
                    }
                    Cmd::Quit(force) => {
                        if !force {
                            let dirty: Vec<_> = buffers
                                .iter()
                                .enumerate()
                                .filter(|(i, b)| !b.undo.is_empty() && *i != SCRATCH.idx)
                                .map(|(i, _)| i)
                                .collect();
                            if !dirty.is_empty() {
                                return Err(EditorErr::Msg(format!(
                                    "cant quit dirty buffers: {:?}",
                                    dirty
                                )));
                            }
                        }
                        return Err(EditorErr::Quit);
                    }
                    Cmd::SplitV => {
                        if let Some(idx) = nodes
                            .splits
                            .get(parent.0)
                            .unwrap()
                            .children
                            .iter()
                            .position(|x| *x == NodeIdx::Leaf(lidx))
                        {
                            let (l, new_parent) =
                                nodes.new_split(vidx, parent, Direction::Vertical, None);
                            let vidx = views.push(View::new(SCRATCH));
                            let comp: Box<dyn Component> = Box::new(vidx);
                            nodes.new_leaf(comp, new_parent, None);
                            nodes
                                .splits
                                .get_mut(parent.0)
                                .unwrap()
                                .children
                                .swap_remove(idx);
                            enter_normal(focus, l, cmd_line);
                            nodes.recalc(parent);
                        }
                    }
                    Cmd::SplitH => {
                        if let Some(idx) = nodes
                            .splits
                            .get(parent.0)
                            .unwrap()
                            .children
                            .iter()
                            .position(|x| *x == NodeIdx::Leaf(lidx))
                        {
                            let (l, new_parent) =
                                nodes.new_split(vidx, parent, Direction::Horizontal, None);
                            let vidx = views.push(View::new(SCRATCH));
                            let comp: Box<dyn Component> = Box::new(vidx);
                            nodes.new_leaf(comp, new_parent, None);
                            nodes
                                .splits
                                .get_mut(parent.0)
                                .unwrap()
                                .children
                                .swap_remove(idx);
                            enter_normal(focus, l, cmd_line);
                            nodes.recalc(parent);
                        }
                    }
                    Cmd::Split => {
                        let vidx = views.push(View::new(SCRATCH));
                        let comp: Box<dyn Component> = Box::new(vidx);
                        nodes.new_leaf(comp, parent, None);
                        enter_normal(focus, lidx, cmd_line);
                    }
                    Cmd::Noop => {}
                }
                Ok(())
            }
            Ok(())
        }
        Focus::Leaf(lidx) => {
            unsafe {
                //UNSAFE but its fine probably :D
                let l = nodes.leaves.get_unchecked_mut(lidx.0);
                let mut comp = ptr::read(&l.comp);
                let lidx = lidx.clone();
                let r = comp.behaviour(key, focus, cmd_line, views, buffers, nodes);
                ptr::write(&mut nodes.leaves.get_unchecked_mut(lidx.0).comp, comp);
                r?
            }
            Ok(())
        }
    }
}

const SCRATCH: BufferIdx = BufferIdx { idx: 0 };
fn main() -> io::Result<()> {
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut nodes = Nodes::new();
    let (width, height) = terminal::size().unwrap();
    let mut cmd_line = CmdLine::new(width, height - 1);
    let root = nodes.new_root(
        Rect {
            x: 0,
            y: 0,
            height: height - 1,
            width: width - 1,
            constraint: Constraints {
                min_height: None,
                max_height: None,
                min_width: None,
                max_width: None,
            },
        },
        Direction::Vertical,
    );
    nodes.new_root(
        //overlay ROOT DO NOT REMOVE
        Rect {
            x: 0,
            y: 0,
            height: height - 1,
            width: width - 1,
            constraint: Constraints {
                min_height: None,
                max_height: None,
                min_width: None,
                max_width: None,
            },
        },
        Direction::Vertical,
    );
    let mut focus = {
        buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
        let args: Vec<String> = env::args().skip(1).collect();
        let bidx = {
            if args.is_empty() {
                SCRATCH
            } else {
                buffers.push(Buffer::new(Some(&args[0]), 0).unwrap())
            }
        };
        let vidx = views.push(View::new(bidx));
        let comp: Box<dyn Component> = Box::new(vidx);
        let l = nodes.new_leaf(comp, root, None);
        Focus::Leaf(l)
    };
    enable_raw_mode()?;
    execute!(
        stdout(),
        terminal::EnterAlternateScreen,
        cursor::SetCursorStyle::SteadyBlock
    )?;

    //inital draw
    let mut old = ScreenBuffer {
        width,
        height,
        cells: vec![
            Cell {
                c: '_',
                fg: FG,
                bg: BG,
            };
            (width * height) as usize
        ], //some placeholder to ensure every cell is overwritten
    };
    let mut new = ScreenBuffer {
        width,
        height,
        cells: vec![
            Cell {
                c: ' ',
                fg: FG,
                bg: BG,
            };
            (width * height) as usize
        ],
    };
    nodes.paint(&focus, &cmd_line, &views, &buffers, &mut old, &mut new)?;
    stdout().flush().unwrap();

    loop {
        match read()? {
            Event::Key(event) => {
                match key_to_exec(
                    event,
                    &mut nodes,
                    &mut focus,
                    &mut cmd_line,
                    &mut views,
                    &mut buffers,
                ) {
                    Err(e) => {
                        match e {
                            EditorErr::Msg(msg) => cmd_line.error(&msg),
                            EditorErr::Dirty(idx) => {
                                cmd_line.error(&format!("buffer:{} is dirty", idx.idx))
                            }
                            EditorErr::InvalidBuffer => cmd_line.error("index is invalid"),
                            EditorErr::ReadOnly(idx) => {
                                cmd_line.error(&format!("buffer:{}is read only", idx.idx))
                            }
                            EditorErr::InvalidFocus => cmd_line.error("invalid focus"),
                            EditorErr::Log(msg) => log(&msg),
                            EditorErr::Io(_) => {
                                log("IO error");
                                break;
                            }
                            EditorErr::Quit => break,
                        }
                        let l = {
                            let mut curr = NodeIdx::Split(SplitIdx(0));
                            loop {
                                match curr {
                                    NodeIdx::Split(s) => {
                                        let Split {
                                            children, focus: f, ..
                                        } = nodes.splits.get(s.0).unwrap();
                                        curr = *children.get(*f).unwrap();
                                    }
                                    NodeIdx::Leaf(l) => break l,
                                }
                            }
                        };
                        focus = Focus::Leaf(l);
                        cmd_line.error = false;
                        queue!(stdout(), SetCursorStyle::SteadyBlock)?;
                    }
                    Ok(_) => {}
                }
                queue!(stdout(), cursor::Hide)?;
                nodes.paint(&focus, &cmd_line, &views, &buffers, &mut old, &mut new)?;
                queue!(stdout(), cursor::Show)?;
                stdout().flush()?;
            }
            Event::Resize(width, height) => {
                cmd_line.rect = Rect {
                    x: 0,
                    y: height - 1,
                    height: 1,
                    width,
                    constraint: Constraints {
                        min_height: Some(1),
                        max_height: Some(1),
                        min_width: Some(width),
                        max_width: Some(width),
                    },
                };
                old = ScreenBuffer {
                    width,
                    height,
                    cells: vec![
                        Cell {
                            c: '_',
                            fg: FG,
                            bg: BG,
                        };
                        (width * height) as usize
                    ],
                    //some placeholder to ensure all cells are overwritten
                };
                new = ScreenBuffer {
                    width,
                    height,
                    cells: vec![
                        Cell {
                            c: ' ',
                            fg: FG,
                            bg: BG,
                        };
                        (width * height) as usize
                    ],
                };
                for r in nodes.roots.clone() {
                    let root = nodes.splits.get_mut(r.0).unwrap();
                    root.rect.width = width - 1;
                    root.rect.height = height - 1;
                    nodes.recalc(r);
                }
                queue!(stdout(), cursor::Hide)?;
                nodes.paint(&focus, &cmd_line, &views, &buffers, &mut old, &mut new)?;
                queue!(stdout(), cursor::Show)?;
                stdout().flush()?;
            }
            _ => {}
        }
    }
    disable_raw_mode().unwrap();
    execute!(stdout(), terminal::LeaveAlternateScreen).unwrap();
    Ok(())
}
