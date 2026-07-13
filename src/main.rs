use crossterm::{
    cursor::{self, SetCursorStyle},
    event::{Event, KeyCode, KeyEvent, KeyModifiers, read},
    execute, queue,
    terminal::{self, disable_raw_mode, enable_raw_mode},
};
use ropey::Rope;
use std::{
    collections::{HashMap, VecDeque},
    env,
    fs::{self, File, OpenOptions},
    io,
    io::{BufReader, Write, stdout},
    panic,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::Mutex,
    usize, vec,
};

mod commandline;
use commandline::{Dummy as CmdLineDummy, cmd_line::CmdLine};

mod ui;
use ui::{
    FG, BG, SELECTION,
    screen::{ScreenBuffer, Cell},
    Nodes,
    LeafIdx,
    Split,
    SplitIdx,
    Rect,
    Position,
    NodeIdx,
    Direction,
    Dimension,
    Constraints,
};

use crate::{commandline::cmd_line, ui::Anchors};

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
fn paste_system_clipboard() -> io::Result<String> {
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
    fn check_flag(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }
    fn new(path: Option<&str>, flags: u64) -> std::io::Result<Buffer> {
        let mut f = flags;
        let buf = if let Some(p) = path {
            let path = PathBuf::from(p);
            if path.exists() && path.is_file() {
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

struct Clipboard{
    clipboard: Option<String>,
}

struct View {
    selection: Option<(usize, usize)>,
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
            cursor: 0,
            prefered_x: 0,
            off: 0,
            mode: Mode::Normal,
        }
    }
    fn scroll(&mut self, rect: &ui::Rect, buffer: &mut Buffer) {
        let line = buffer.buf.char_to_line(self.cursor);
        let height = {
            let mut wrap_off = 0usize;
            for line_idx in self.off..line {
                if let Some(line) = buffer.buf.get_line(line_idx) {
                    let len = line.len_chars();
                    if len > 0 {
                        if rect.w.saturating_sub(5) == 0 {
                            panic!("width:{}, height:{}", rect.w, rect.h)
                        }
                        wrap_off += len / rect.w.saturating_sub(5) as usize;
                    }
                }
            }
            let height = rect.h.saturating_sub(2) as usize;
            height.saturating_sub(wrap_off)
        };
        if line < self.off {
            self.off = line;
        } else if line > self.off + height {
            self.off = line - height;
        }
    }
}

trait Component {
    fn sketch(
        &self,
        rect: &ui::Rect,
        views: &Views,
        buffers: &Buffers,
        cmd_line: &CmdLine,
        screen: &mut ui::screen::ScreenBuffer,
        cwd: &PathBuf,
        focus: &ui::LeafIdx,
    );
    fn cursor_xy(
        &self,
        rect: &ui::Rect,
        views: &Views,
        buffers: &Buffers,
        cmd_line: &CmdLine,
        nodes: &ui::Nodes,
    ) -> (u16, u16, SetCursorStyle);
    fn behaviour(
        &mut self,
        key: KeyEvent,
        focus: &mut ui::LeafIdx,
        cmd_line: &mut CmdLine,
        views: &mut Views,
        buffers: &mut Buffers,
        nodes: &mut ui::Nodes,
        cwd: &mut PathBuf,
        clipboard: &mut Clipboard,
    ) -> Result<(), EditorErr>;
}

impl Component for ViewIdx {
    fn sketch(
        &self,
        rect: &ui::Rect,
        views: &Views,
        buffers: &Buffers,
        _cmd_line: &CmdLine,
        screen: &mut ui::screen::ScreenBuffer,
        cwd: &PathBuf,
        _focus: &ui::LeafIdx,
    ) {
        let v = views.get(*self);
        {
            let blank = " ".repeat(rect.w as usize);
            for row in 0..rect.h {
                //clear text area, line num area, status line
                screen.set_string_xy(rect.x, rect.y + row, &blank, FG, BG);
            }
        }
        deco_sketch(v, rect, buffers, screen, cwd);
        text_sketch(v, rect, buffers, screen);
        selection_sketch(v, rect, buffers, screen);
        fn text_sketch(view: &View, rect: &Rect, buffers: &Buffers, screen: &mut ScreenBuffer) {
            let width = rect.w.saturating_sub(5) as usize;
            let height = rect.h.saturating_sub(1) as usize;
            let mut row = 0;
            let mut line_idx = view.off;
            while row <= height {
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
        fn deco_sketch(view: &View, rect: &Rect, buffers: &Buffers, screen: &mut ScreenBuffer, cwd: &PathBuf) {
            let wrap_width = rect.w.saturating_sub(4) as usize;

            let mut screen_row = 0usize;
            let mut line_idx = view.off;

            while screen_row < rect.h as usize {
                let Some(line) = buffers.get(view.buf).buf.get_line(line_idx) else {
                    break;
                };

                let line_len = line.len_chars();
                let visual_rows = usize::max(1, line_len.div_ceil(wrap_width));

                for visual_row in 0..visual_rows {
                    if screen_row >= rect.h as usize {
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
            let mut path = format!("[SCRATCH] {}",cwd.display());
            let buffer = buffers.get(view.buf);
            if !buffer.check_flag(Buffer::SCRATCH) {
                if let Some(p) = &buffer.file {
                    path = p.display().to_string();
                        // .unwrap_or(format!("[NEW_FILE] {}",cwd.into()));
                } else {
                    path = format!("[NEW_FILE] {}",cwd.display());
                }
            }
            let mode_str = match view.mode {
                Mode::Normal => "NOR",
                Mode::Insert => "INS",
                Mode::Visual => "VIS",
            };
            let s = format!("{mode_str} {} {path}", view.buf.idx);
            screen.set_string_xy(rect.x, rect.y + rect.h.saturating_sub(1), &s, FG, BG);
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
            let sel_start = a;
            let sel_end = b;
            let width = rect.w.saturating_sub(5) as usize;
            let height = rect.h.saturating_sub(1) as usize;
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
    fn cursor_xy(
        &self,
        rect: &Rect,
        views: &Views,
        buffers: &Buffers,
        _cmd_line: &CmdLine,
        _nodes: &Nodes,
    ) -> (u16, u16, SetCursorStyle) {
        let v = views.get(*self);
        let b = buffers.get(v.buf);
        let width = rect.w.saturating_sub(4) as usize;
        if width == 0 {
            return (rect.x + 5, rect.y, SetCursorStyle::SteadyBar);
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
        match v.mode {
            Mode::Normal => (
                rect.x + x as u16 + 5,
                rect.y + nested_y as u16,
                SetCursorStyle::SteadyBlock,
            ),

            Mode::Insert => (
                rect.x + x as u16 + 5,
                rect.y + nested_y as u16,
                SetCursorStyle::SteadyBar,
            ),
            Mode::Visual => (
                rect.x + x as u16 + 5,
                rect.y + nested_y as u16,
                SetCursorStyle::SteadyUnderScore,
            ),
        }
    }
    fn behaviour(
        &mut self,
        key: KeyEvent,
        focus: &mut LeafIdx,
        cmd_line: &mut CmdLine,
        views: &mut Views,
        buffers: &mut Buffers,
        nodes: &mut Nodes,
        cwd: &mut PathBuf,
        clipboard: &mut Clipboard,
    ) -> Result<(), EditorErr> {
        let cmd = key_to_cmd(key, views.get(*self));
        exec_cmd(cmd, *self, nodes, focus, cmd_line, views, buffers, cwd, clipboard)?;
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
                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::ALT) => Cmd::EnterCmd,
                    KeyCode::Char('K') => Cmd::FocusUp,
                    KeyCode::Char('u') => Cmd::Undo,
                    KeyCode::Char('U') => Cmd::Redo,
                    KeyCode::Char('h') => Cmd::MoveLeft,
                    KeyCode::Char('j') => Cmd::MoveDown,
                    KeyCode::Char('k') => Cmd::MoveUp,
                    KeyCode::Char('l') => Cmd::MoveRight,
                    KeyCode::Char('H') => Cmd::FocusLeft,
                    KeyCode::Char('J') => Cmd::FocusDown,
                    KeyCode::Char('L') => Cmd::FocusRight,
                    KeyCode::Char('p') => Cmd::Paste,
                    KeyCode::Char('P') => Cmd::PasteClipboard,
                    KeyCode::Char('v') => Cmd::EnterVisual,
                    KeyCode::Char('d') => Cmd::BackSpace,
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
                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::ALT) => Cmd::EnterCmd,
                    KeyCode::Char('K') => Cmd::FocusUp,
                    KeyCode::Char('i') => Cmd::EnterInsert,
                    KeyCode::Char('k') => Cmd::MoveSelectionUp,
                    KeyCode::Char('j') => Cmd::MoveSelectionDown,
                    KeyCode::Char('h') => Cmd::MoveSelectionLeft,
                    KeyCode::Char('l') => Cmd::MoveSelectionRight,
                    KeyCode::Char('y') => Cmd::Yank,
                    KeyCode::Char('Y') => Cmd::YankClipboard,
                    KeyCode::Char('p') => Cmd::Paste,
                    KeyCode::Char('P') => Cmd::PasteClipboard,
                    KeyCode::Char('d') => Cmd::BackSpace,
                    _ => Cmd::Noop,
                },
            }
        }
        fn exec_cmd(
            cmd: Cmd,
            vidx: ViewIdx,
            nodes: &mut Nodes,
            focus: &mut LeafIdx,
            cmd_line: &mut CmdLine,
            views: &mut Views,
            buffers: &mut Buffers,
            cwd: &mut PathBuf,
            clipboard: &mut Clipboard,
        ) -> Result<(), EditorErr> {
            fn enter_normal(view: &mut View, cmd_line: &mut CmdLine) {
                view.mode = Mode::Normal;
                view.selection = None;
                cmd_line.cursor = 0;
            }
            let (bidx, lidx) = {
                let mut curr = NodeIdx::Split(SplitIdx(0));
                let lidx = loop {
                    match curr {
                        NodeIdx::Split(s) => {
                            let Split {
                                children, focus: f, ..
                            } = nodes.get_split(s);
                            curr = children[*f];
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
                    v.selection = Some((v.cursor, v.cursor + 1));
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
                        debug_assert!(None != view.selection, "should only be used while in visual mode");
                        if view.cursor >= view.selection.unwrap().0 {
                            view.selection.as_mut().unwrap().1 =
                                usize::min(view.cursor + 1, buffer.buf.len_chars());
                        } else {
                            view.selection.as_mut().unwrap().0 =
                                usize::min(view.cursor, buffer.buf.len_chars());
                        }
                    }
                    let buffer = buffers.get_mut(bidx);
                    View::scroll(view, &nodes.get_leaf(lidx).rect, buffer);
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
                        debug_assert!(None != view.selection, "should only be used while in visual mode");
                        if view.cursor >= view.selection.unwrap().1 {
                            view.selection.as_mut().unwrap().1 =
                                usize::min(view.cursor + 1, buffer.buf.len_chars());
                        } else {
                            view.selection.as_mut().unwrap().0 =
                                usize::min(view.cursor, buffer.buf.len_chars());
                        }
                        View::scroll(view, &nodes.get_leaf(lidx).rect, buffers.get_mut(bidx));
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
                        debug_assert!(None != view.selection, "should only be used while in visual mode");
                        if view.cursor >= view.selection.unwrap().1 {
                            view.selection.as_mut().unwrap().1 =
                                usize::min(view.cursor + 1, buffer.buf.len_chars());
                        } else {
                            view.selection.as_mut().unwrap().0 =
                                usize::min(view.cursor, buffer.buf.len_chars());
                        }
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
                        debug_assert!(None != view.selection, "should only be used while in visual mode");
                        if view.cursor >= view.selection.unwrap().0 {
                            view.selection.as_mut().unwrap().1 =
                                usize::min(view.cursor + 1, buffer.buf.len_chars());
                        } else {
                            view.selection.as_mut().unwrap().0 =
                                usize::min(view.cursor, buffer.buf.len_chars());
                        }
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
                    clipboard.clipboard = Some(b.buf.slice(selection.0..selection.1).to_string());
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
                    let selection = &b.buf.slice(selection.0..selection.1).to_string();
                    yank_to_system_clipboard(selection).unwrap(); //kinda slow due to syscall and beeing blocking
                    v.selection = None;
                    enter_normal(v, cmd_line);
                }
                Cmd::Paste => {
                    if let Some(line) = &clipboard.clipboard{
                        let v = views.get_mut(vidx);
                        let b = buffers.get_mut(v.buf);
                        let idx = usize::min(v.cursor + 1, b.buf.len_chars());
                        b.undo.push(Edit::Insert { idx, text: line.clone()});
                        v.cursor += line.chars().count();
                        v.selection = None;
                    };
                }
                Cmd::PasteClipboard => {
                    let s = match paste_system_clipboard() {
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
                    cmd_line.enter_cmd_mode(vidx, focus, views, lidx, buffers, nodes, cwd);
                }
                Cmd::EnterInsert => {
                    let v = views.get_mut(vidx);
                    v.mode = Mode::Insert;
                    v.selection = None;
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
                    View::scroll(view, &nodes.get_leaf(lidx).rect, buffer);
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
                        View::scroll(view, &nodes.get_leaf(lidx).rect, buffers.get_mut(bidx));
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
                        View::scroll(view, &nodes.get_leaf(lidx).rect, buffer);
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
                    View::scroll(v, &nodes.get_leaf(lidx).rect, buffer);
                }
                Cmd::BackSpace => {
                    fn backspace(
                        views: &mut Views,
                        vidx: ViewIdx,
                        bidx: BufferIdx,
                        buffers: &mut Buffers,
                        nodes: &mut Nodes,
                        lidx: LeafIdx,
                    ) {
                        let v = views.get_mut(vidx);
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
                            v.cursor = usize::min(v.cursor, buffer.buf.len_chars().saturating_sub(1));
                            View::scroll(v, &nodes.get_leaf(lidx).rect, buffer);
                        }
                    }
                    let v = views.get_mut(vidx);
                    if let Some(sel) = v.selection {
                        v.cursor = usize::min(sel.1, buffers.get(bidx).buf.len_chars());
                        v.mode = Mode::Normal;
                        for _ in sel.0..sel.1 {
                            backspace(views, vidx, bidx, buffers, nodes, lidx);
                        }
                        views.get_mut(vidx).selection = None;
                    } else {
                        if Mode::Normal == v.mode{
                            let b = buffers.get(v.buf);
                            v.cursor = usize::min(v.cursor+1, b.buf.len_chars());
                        }
                        backspace(views, vidx, bidx, buffers, nodes, lidx);
                    }
                }
                Cmd::Noop => {}
            }
            Ok(())
        }
        Ok(())
    }
}


#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Insert,
    Visual,
}

fn key_to_exec(
    key: KeyEvent,
    nodes: &mut Nodes,
    focus: &mut LeafIdx,
    cmd_line: &mut CmdLine,
    views: &mut Views,
    buffers: &mut Buffers,
    cmd: &mut PathBuf,
    clipboard: &mut Clipboard,
) -> Result<(), EditorErr> {
    unsafe {
        //UNSAFE but its fine probably :D
        let l = nodes.get_leaf(*focus);
        let mut comp = ptr::read(&l.comp);
        let lidx = focus.clone();
        let r = comp.behaviour(key, focus, cmd_line, views, buffers, nodes, cmd, clipboard);
        ptr::write(&mut nodes.get_mut_leaf(lidx).comp, comp);
        r?
    }
    Ok(())
}

const SCRATCH: BufferIdx = BufferIdx { idx: 0 };
const CMDLINE: LeafIdx = LeafIdx(1);
//index into nodes.roots
const ROOT_TEXT_VIEW: usize = 0;
const ROOT_CMD_LINE: usize = 1;
const ROOT_OVERLAY: usize = 2;
fn main() -> io::Result<()> {
    let mut cwd = env::current_dir().unwrap();
    cmd_line::check_alias_collison();
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut nodes = Nodes::new();
    let mut clipboard = Clipboard{clipboard:None};
    let (width, height) = terminal::size().unwrap();
    let root = nodes.new_root(
        Constraints{
            min_height: None,
            max_height: Some(vec![Dimension::AddRelative(1), Dimension::SubAbsolute(1)]),
            min_width: None,
            max_width: None,
        },
        Anchors::new(),
        width, height,
        Direction::Vertical
    );
    nodes.new_root(
        Constraints::new(),
        Anchors::new(),
        width, height,
        Direction::Horizontal,
    );
    nodes.new_root(Constraints::new(), Anchors::new(), width, height, Direction::Vertical);
    // nodes.recalc_including_root(width, height);
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
        nodes.new_leaf(comp, root, Constraints::new(), Anchors::new())
    };

    let mut cmd_line = CmdLine::new();
    let comp: Box<dyn Component> = Box::new(CmdLineDummy());
    nodes.new_leaf(comp, nodes.get_root(ROOT_CMD_LINE),
    Constraints{
        min_height: None,
        max_height: Some(vec![Dimension::AddAbsolute(1)]),
        min_width: None,
        max_width: None,
    },
    Anchors{x:None, y:Some(vec![Position::AddRelative(1), Position::SubAbsolute(1)])},
    );
    enable_raw_mode()?;
    execute!(stdout(), terminal::EnterAlternateScreen)?;

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

    nodes.paint(
        &focus, &cmd_line, &views, &buffers, &mut old, &mut new, &nodes, &cwd
    )?;
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
                    &mut cwd,
                    &mut clipboard,
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
                            EditorErr::Log(msg) => log(&msg),
                            EditorErr::Io(e) => {
                                log(&format!("IO error: {e}"));
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
                                        } = nodes.get_split(s);
                                        curr = *children.get(*f).unwrap();
                                    }
                                    NodeIdx::Leaf(l) => break l,
                                }
                            }
                        };
                        focus = l;
                        cmd_line.error = false;
                    }
                    Ok(_) => {}
                }
                queue!(stdout(), cursor::Hide)?;
                nodes.paint(
                    &focus, &cmd_line, &views, &buffers, &mut old, &mut new, &nodes, &cwd,
                )?;
                queue!(stdout(), cursor::Show)?;
                stdout().flush()?;
            }
            Event::Resize(width, height) => {
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
                nodes.recalc_including_root(width, height);
                queue!(stdout(), cursor::Hide)?;
                nodes.paint(
                    &focus, &cmd_line, &views, &buffers, &mut old, &mut new, &nodes, &cwd
                )?;
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
