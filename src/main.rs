use std::collections::HashMap;
use std::fs::canonicalize;
use std::collections::VecDeque;
use std::collections::hash_map;
use std::fs::{self, File};
use std::path::Path;
use std::process::{exit};
use ropey::Rope;
use std::path::PathBuf;
use std::{env, io};
use std::io::{BufReader, Write};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::event::Key;

impl From<std::io::Error> for EditorErr{
    fn from(e: std::io::Error)->Self{
        EditorErr::Io(e)
    }
}
#[derive(Debug)]
enum EditorErr{
    Io(std::io::Error),
    ReadOnly(BufferIdx),
    InvalidBuffer,
    Dirty(BufferIdx),
    Msg(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferIdx{
    idx:usize,
    generation:u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewIdx{
    idx:usize,
    generation:u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GroupIdx{
    idx:usize,
    generation:u64,
}

struct Buffers{
    data:Vec<Buffer>,
    free:VecDeque<BufferIdx>,
    path_map:HashMap<PathBuf, BufferIdx>,
}
impl Buffers{
    fn new()->Self{
        Self{
            data: Vec::new(),
            free: VecDeque::new(),
            path_map: HashMap::new(),
        }
    }
    fn get(&self, idx: BufferIdx)->&Buffer{
        &self.data[idx.idx]
    }
    fn get_mut(&mut self, idx: BufferIdx)->&mut Buffer{
        &mut self.data[idx.idx]
    }
    fn push(&mut self, buf: Buffer) -> BufferIdx {
        let idx = if self.free.is_empty(){
            let idx = BufferIdx{idx:self.data.len(), generation:0};
            self.data.push(buf.clone());
            idx
        }else{
            let mut idx = self.free.pop_front().unwrap();
            idx.generation += 1;
            let element = self.get_mut(idx);
            *element = buf.clone();
            idx
        };
        if let Some(p) = buf.file{
            if let Ok(path) = p.canonicalize(){
                self.path_map.insert(path, idx);
            }
        }
        idx
    }
    fn get_by_path(&self, path: &str)->Option<&BufferIdx>{
        if let Ok(p) = Path::new(path).canonicalize(){
            let buffer = self.path_map.get(&p);
            if let Some(idx) = buffer{
                if idx.generation == self.get(*idx).generation{
                    return Some(idx)
                }
            }
        }
        None
    }
    fn remove(&mut self, idx: &mut BufferIdx){
        self.get_mut(*idx).generation += 1;
        idx.generation += 1;
        self.data[idx.idx].partial_reset();
        self.free.push_back(*idx);
    }
    fn len(&self)->usize{
        self.data.len()
    }
    fn iter(&self)->impl Iterator<Item = &Buffer>{
        self.data.iter()
    }
    fn iter_mut(&mut self)->impl Iterator<Item = &mut Buffer>{
        self.data.iter_mut()
    }
}
struct Views(Vec<View>);
impl Views{
    fn new()->Self {Self(Vec::new())}
    fn get(&self, idx: ViewIdx)->&View{
        &self.0[idx.idx]
    }
    fn get_mut(&mut self, idx: ViewIdx)->&mut View{
        &mut self.0[idx.idx]
    }
    fn push(&mut self, view: View) -> ViewIdx{
        let idx = ViewIdx{idx:self.0.len(), generation: 0};
        self.0.push(view);
        idx
    }
    fn len(&self)->usize{
        self.0.len()
    }
    fn iter(&self)->impl Iterator<Item = &View>{
        self.0.iter()
    }
    fn iter_mut(&mut self)->impl Iterator<Item = &mut View>{
        self.0.iter_mut()
    }
}
struct Groups(Vec<Group>);
impl Groups{
    fn new()->Self {Self(Vec::new())}
    fn get(&self, idx:GroupIdx)->&Group{
        &self.0[idx.idx]
    }
    fn get_mut(&mut self, idx: GroupIdx)->&mut Group{
        &mut self.0[idx.idx]
    }
    fn push(&mut self, group: Group) -> GroupIdx{
        let idx = GroupIdx { idx:self.0.len(), generation: 0};
        self.0.push(group);
        idx
    }
    fn len(&self)->usize{
        self.0.len()
    }
    fn iter(&self)->impl Iterator<Item = &Group>{
        self.0.iter()
    }
    fn iter_mut(&mut self)->impl Iterator<Item = &mut Group>{
        self.0.iter_mut()
    }
}

#[derive(Clone)]
struct Buffer{
    generation: u64,
    flags: u64,
    file: Option<PathBuf>,
    buf: Rope,
}

impl Buffer{
    const READ_ONLY:       u64 = 1 << 0;
    const SCRATCH:         u64 = 1 << 1;
    const DIRTY:           u64 = 1 << 2;
    const NEW_FILE:        u64 = 1 << 3;
    const NON_NAVIGATABLE: u64 = 1 << 4;
    // const EMPTY:           u64 = 1 << 5;
    fn partial_reset(&mut self){
        self.buf = Rope::new();
        //does not reset flags or pathbuf
    }
    fn set_flag(&mut self, flag: u64){
        self.flags |= flag
    }
    fn clear_flag(&mut self, flag: u64){
        self.flags &= !flag
    }
    fn check_flag(&self, flag: u64)->bool{
        self.flags & flag != 0
    }
    fn new(path: Option<&str>, flags: u64)->std::io::Result<Buffer>{
        let mut f = flags;
        let buf = if let Some(p) = path {
            let path = PathBuf::from(p);
            if path.exists(){
                let cont = fs::read_to_string(&path)?;
                if fs::metadata(&path)?.permissions().readonly(){
                    f |= Self::READ_ONLY;
                }
                Rope::from_str(&cont)
            }else{
                f |= Self::NEW_FILE;
                Rope::new()
            }
        }else{
                f |= Self::NEW_FILE;
                Rope::new()
        };
        Ok(Buffer{
            generation: 0,
            flags: f,
            buf: buf,
            file: path.map(PathBuf::from),
        })
    }
    fn insert(&mut self, view: &View, c: char){
        let cursor_char = view.cursor_char(self);
        self.buf.insert_char(cursor_char, c);
        self.set_flag(Buffer::DIRTY);
    }
    fn save(&mut self, new: Option<String>)->io::Result<()>{
        if let Some(new) = new{
            let file = File::create(new)?;
            self.buf.write_to(file)?;
        }else{
            if let Some(path) = &self.file{
                let file = File::create(path)?;
                self.buf.write_to(file)?;
            }
        }
        self.clear_flag(Buffer::DIRTY);
        Ok(())
    }
}

struct CmdLine{
    input: String,
    pos_y: u16,
    cursor: usize,
}
impl CmdLine{
    fn new(height: u16)->Self{
        Self{
            input: String::new(),
            pos_y: height,
            cursor: 0,
        }
    }
    fn insert(&mut self, c: char){
        let byte_idx = self.cursor;
        self.input.insert(byte_idx, c);
        self.cursor += c.len_utf8();
    }
    fn backspace(&mut self){
        if self.cursor > 0 {
            let char_len = self.input[..self.cursor].chars().rev().next().unwrap().len_utf8();
            self.cursor -= char_len as usize;
            self.input.remove(self.cursor);
        }
    }
    fn draw_error(&mut self, mode: &mut Mode, s: &str)->io::Result<()>{
        let mut out = io::stdout().lock();
        self.cursor = 0;
        self.input.clear();
        *mode = Mode::Normal;
        write!(out, "{}{}",termion::cursor::Goto(1, self.pos_y+1), s)?;
        Ok(())
    }
    fn draw(&self, mode: Mode)->io::Result<()>{
        let mut out = io::stdout().lock();
        match mode{
            Mode::Command =>{
                write!(out, "{}{}",termion::cursor::Goto(1, self.pos_y+1), termion::clear::CurrentLine)?;
                write!(out, "{}:{}",termion::cursor::Goto(1,self.pos_y+1), self.input)?;
            }
            Mode::Normal | Mode::Insert => write!{out, "{}{}",termion::cursor::Goto(1, self.pos_y+1), termion::clear::CurrentLine}?,
        }
        out.flush()
    }
}

struct View{
    buf: Option<BufferIdx>,
    y: usize,
    x: usize,
    prefered_x: usize,
    off: usize,
    width: u16,
    height: u16,
    pos_x: u16,
    pos_y: u16,
    flags: u16,
}

impl View{
    const NON_NAVIGATABLE: u16 = 1 << 0;
    // const FLOATING:        u16 = 1 << 1;
    const LINE_NUMBER:     u16 = 1 << 2;
    const STATUS_BAR:      u16 = 1 << 3;
    // const EMPTY:           u16 = 1 << 4;
    fn check_flag(&self, flag: u16)->bool{
        self.flags & flag != 0
    }
    fn new(buf: Option<BufferIdx>, pos_x: u16, pos_y: u16, width: u16, height: u16, flags:u16)->Self{
        Self{
            buf,
            x: 0,
            prefered_x: 0,
            y: 0,
            off: 0,
            pos_x,
            pos_y,
            width,
            height,
            flags,
        }
    }
    fn redraw(&self)->io::Result<()>{
        let mut out = io::stdout().lock();
        write!(out, "{}",termion::clear::All)?;
        Ok(())
    }
    fn draw_status_bar(&self, idx: BufferIdx, buffers: &Buffers, mode: Mode)->io::Result<()>{
        let buffer = buffers.get(idx);
        let mut out = io::stdout().lock();
        write!(out, "{}", termion::cursor::Goto(self.pos_x+1, self.pos_y+1))?;
        let mut path = "SCRATCH";
        if !buffer.check_flag(Buffer::SCRATCH){
            if let Some(p) = &buffer.file{
                path = p.to_str().unwrap();
            }else{
                path = "NEW_FILE";
            }
        }
        let mode_str = match mode{
            Mode::Command => "CMD",
            Mode::Insert  => "INS",
            _ => "NOR",
        };
        write!(out, "{mode_str} {} {path}", idx.idx)?;
        Ok(())
    }
    fn draw_line_numbers(&self) -> io::Result<()> {
        let mut out = io::stdout().lock();

        let start = self.off;
        let height = self.height as usize;
        let width = self.width as usize;

        for row in 0..height+1{
            let screen_y = self.pos_y + row as u16 + 1;
            let line_num = start + row + 1;

            write!( out, "{}", termion::cursor::Goto(self.pos_x + 1, screen_y))?;
            write!(out, "{:>width$} ", line_num, width = width.saturating_sub(1))?;
        }
        Ok(())
    }
    fn draw_text(&self, buffers: &Buffers) -> io::Result<()>{
        let buffer = if let Some(b) = self.buf{
            b
        }else{
            BufferIdx { idx: 0, generation: 0}
        };
        let buffer = buffers.get(buffer);
        let mut out = io::stdout().lock();
        let start = self.off;
        let end = usize::min(start + self.height as usize, buffer.buf.len_lines())+1;

        for row in 0..(end - start){
            write!(out, "{}", termion::cursor::Goto(self.pos_x+1, self.pos_y + row as u16 + 1))?;
            let line_index = start + row;
            if let Some(line) = buffer.buf.get_line(line_index){
                let end = usize::min(self.width as usize, line.len_chars());
                let slice = line.slice(..end.saturating_sub(1));//off by one if not -1 totally didnt spend 2 days trying to find it
                write!(out, "{}",slice)?;
                let remaining = self.width as usize - slice.len_chars();
                for _ in 0..remaining{
                    write!(out, " ")?;
                }
            }else{
                for _ in 0..self.width{
                    write!(out, " ")?;
                }
            }
        }
        let screen_y = self.pos_y + self.y.saturating_sub(self.off) as u16;
        let screen_x = self.pos_x + self.x as u16;
        write!(out, "{}", termion::cursor::Goto(screen_x+1, screen_y+1))?;
        out.flush()?;
        Ok(())
    }
    fn scroll(&mut self, buffer: &mut Buffer){
        if self.y < self.off{
            self.off = self.y;
        } else if self.y >= self.off + self.height as usize{
            self.off = self.y - self.height as usize;
        }
        if let Some(line) = buffer.buf.get_line(self.y){
            if line.len_chars() > 0 {
                self.x = usize::min(self.x, line.len_chars().saturating_sub(1));
            }else{
                self.x = 0;
            }
        }else{
            self.x = 0;
        }
    }
    fn cursor_char(&self, buffer: &Buffer) -> usize {
        buffer.buf.line_to_char(self.y)+self.x
    }
}

struct Group{
    parent: ViewIdx,
    children: Vec<ViewIdx>,
}

impl Group{
    fn new(views: &mut Views, parent: ViewIdx, children_flags:u16)->Self{
        let parent_pos_x = views.get(parent).pos_x;
        let parent_pos_y = views.get(parent).pos_y;
        let mut parent_height = views.get(parent).height;
        let parent_width = views.get(parent).width;
        let mut children = vec![];
        if children_flags & View::STATUS_BAR != 0 {
            let view = views.push(View::new(
                None, parent_pos_x,
                parent_height - parent_pos_y, parent_width,
                1, View::NON_NAVIGATABLE | View::STATUS_BAR));
            children.push(view);
            views.get_mut(parent).height -= 1;
            parent_height -= 1;
        }
        if children_flags & View::LINE_NUMBER != 0 {
            let view = views.push(View::new(
                None, parent_pos_x,
                parent_pos_y, 5,
                parent_height, View::NON_NAVIGATABLE | View::LINE_NUMBER)
            );
            children.push(view);
            views.get_mut(parent).pos_x = views.get(parent).pos_x.saturating_add(5);
            views.get_mut(parent).width = views.get(parent).width.saturating_sub(5);
        }
        Self{
            parent,
            children,
        }
    }
        fn sync(&self, views: &mut Views){
        let (y, off) = {
            let parent = &views.get(self.parent);
            (parent.y, parent.off)
        };
        for &child in &self.children{
            if !views.get(child).check_flag(View::STATUS_BAR){
                let child = &mut views.get_mut(child);
                child.y = y;
                child.off = off;
            }
        }
    }
    fn draw_group(&self, mode: Mode, views: &Views, buffers: &Buffers)->io::Result<()>{
        for c in self.children.iter(){
            let curr = views.get(*c);
            if curr.check_flag(View::STATUS_BAR){
                let parent = views.get(self.parent);
                if let Some(b) = parent.buf{
                    curr.draw_status_bar(b, buffers, mode)?
                }
            }else if curr.check_flag(View::LINE_NUMBER){
                curr.draw_line_numbers()?;
            }else{
                curr.draw_text(buffers)?;
            }
        }
        views.get(self.parent).draw_text(buffers)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Mode{
    Normal,
    Insert,
    Command,
}

enum Cmd{
    CmdInsert(char),
    CmdBackspace,
    CmdMoveLeft,
    CmdMoveRight,
    InsertChar(char),
    NewLine,
    Backspace,
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
    Close(Option<BufferIdx>, bool),
    Open(Option<String>),
    Save(Option<String>),
    Quit(bool),
    SwitchBuffer(BufferIdx),
    EnterModeInsert,
    EnterModeNormal,
    EnterModeCommand,
    NoOp,
}

fn key_to_cmd(key: Key, mode: &Mode)->Cmd{
    match key{
        Key::Esc => Cmd::EnterModeNormal,
        _ => {
            match mode{
                Mode::Normal=>{
                    match key{
                        Key::Char('i') => Cmd::EnterModeInsert,
                        Key::Char(':') => Cmd::EnterModeCommand,
                        Key::Char('k') | Key::Up => Cmd::MoveUp,
                        Key::Char('j') | Key::Down => Cmd::MoveDown,
                        Key::Char('h') | Key::Left => Cmd::MoveLeft,
                        Key::Char('l') | Key::Right => Cmd::MoveRight,
                        _ => Cmd::NoOp,
                    }
                },
                Mode::Insert=>{
                    match key{
                        Key::Up    => Cmd::MoveUp,
                        Key::Down  => Cmd::MoveDown,
                        Key::Left  => Cmd::MoveLeft,
                        Key::Right => Cmd::MoveRight,
                        Key::Char('\n') => Cmd::NewLine,
                        Key::Char(c) if !c.is_control() => Cmd::InsertChar(c),
                        Key::Backspace => Cmd::Backspace,
                        _ => Cmd::NoOp,
                    }
                },
                Mode::Command=>{
                    match key{
                        Key::Left => Cmd::CmdMoveLeft,
                        Key::Right => Cmd::CmdMoveRight,
                        Key::Backspace => Cmd::CmdBackspace,
                        Key::Char(c) => Cmd::CmdInsert(c),
                        _ => Cmd::NoOp
                    }
                }
            }
        }
    }
}

fn exec_cmd(cmd_line: &mut CmdLine, view: ViewIdx, views: &mut Views, buffers: &mut Buffers, groups: &mut Groups, cmd: Cmd, mode: &mut Mode)->Result<(), EditorErr>{
    let bidx = if let Some(b) = views.get(view).buf{
        b
    }else{
        return Err(EditorErr::Msg(format!("invalid buffer: {:?}",views.get(view).buf)));//shouldnt happen
    };
    let buffer = buffers.get_mut(bidx);
    let mut curr_view = views.get_mut(view);
    match cmd{
        Cmd::EnterModeInsert => {
            *mode = Mode::Insert;
            cmd_line.input.clear();
            cmd_line.draw(*mode)?;
            cmd_line.cursor = 0;
            Ok(())
        }
        Cmd::EnterModeNormal  => {
            *mode = Mode::Normal;
            cmd_line.input.clear();
            cmd_line.draw(*mode)?;
            cmd_line.cursor = 0;
            Ok(())
        }
        Cmd::EnterModeCommand => {
            *mode = Mode::Command;
            cmd_line.input.clear();
            cmd_line.draw(*mode)?;
            Ok(())
        }
        Cmd::InsertChar(c)=>{
            buffer.insert(views.get(view), c);
            let view = views.get_mut(view);
            view.x += 1;
            view.prefered_x = view.x;
            View::scroll(view, buffer);
            Ok(())
        },
        Cmd::NewLine=>{
            buffer.insert(&mut curr_view, '\n');
            curr_view.y += 1;
            curr_view.x = 0;
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::Backspace=>{
            let idx = curr_view.cursor_char(buffer);
            if idx != 0{
                buffer.buf.remove(idx - 1..idx);
                if curr_view.x > 0{
                    curr_view.x -= 1;
                    curr_view.prefered_x = curr_view.x;
                }else{
                    curr_view.y = curr_view.y.saturating_sub(1);
                    if let Some(line) = buffer.buf.get_line(curr_view.y){
                        curr_view.x = line.len_chars();
                    }
                }
                View::scroll(&mut curr_view, buffer);
            }
            Ok(())
        },
        Cmd::MoveUp=>{
            curr_view.y = curr_view.y.saturating_sub(1);
            if let Some(line) = buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.prefered_x.min(line.len_chars());
            }
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::MoveDown=>{
            if buffer.buf.len_lines() > 0{
                curr_view.y = usize::min(curr_view.y+1, buffer.buf.len_lines().saturating_sub(1));
            }
            if let Some(line) = buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.prefered_x.min(line.len_chars().saturating_sub(1));
            }
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::MoveRight=>{
            curr_view.x = curr_view.x + 1;
            if let Some(line) = buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.x.min(line.len_chars().saturating_sub(1));
            }
            curr_view.prefered_x = curr_view.x;
            Ok(())
        },
        Cmd::MoveLeft=>{
            curr_view.x = curr_view.x.saturating_sub(1);
            curr_view.prefered_x = curr_view.x;
            Ok(())
        },
        Cmd::CmdInsert(c) => {
            if c != '\n' {
                cmd_line.insert(c);
                cmd_line.draw(*mode)?;
            }else{
                let input = cmd_line.input.clone();
                match parse_cmd(input){
                    Ok(parsed_cmd) => exec_cmd(cmd_line, view, views, buffers, groups, parsed_cmd, mode)?,
                    Err(e) => {
                        cmd_line.draw_error(mode, &format!("{:?}", e))?;
                    }
                }
            }
            Ok(())
        },
        Cmd::CmdBackspace=>{
            cmd_line.backspace();
            cmd_line.draw(*mode)?;
            Ok(())
        },
        Cmd::CmdMoveLeft=>{
            cmd_line.cursor = cmd_line.cursor.saturating_sub(1);
            Ok(())
        }
        Cmd::CmdMoveRight=>{
            cmd_line.cursor = cmd_line.cursor.saturating_add(1);
            Ok(())
        }
        Cmd::Save(f)=>{
            if buffer.check_flag(Buffer::READ_ONLY){
                cmd_line.draw_error(mode, "cannot save read only")?;
                return Err(EditorErr::ReadOnly(bidx));
            }
            if buffer.check_flag(Buffer::SCRATCH){
                return Err(EditorErr::Msg(format!("cant save, buffer: {} is scratch",bidx.idx)));
            }
            if let Some(new) = f{
                buffer.save(Some(new))?;
            }else{
                if let Some(_) = &buffer.file{
                    match buffer.save(None){
                        Err(error)=>return Err(EditorErr::Io(error)),
                        Ok(_)=>{},
                    }
                }else{
                    return Err(EditorErr::Msg("new file needs name".into()));
                }
            }
            cmd_line.input.clear();
            cmd_line.cursor = 0;
            *mode = Mode::Normal;
            Ok(())
        },
        Cmd::Close(buffer_idx, force)=>{
            let mut idx = {
                if let Some(idx) = buffer_idx{
                    idx
                }else{
                    if let Some(buf) = curr_view.buf{
                        buf
                    }else{
                        return Err(EditorErr::InvalidBuffer);
                    }
                }
            };
            let curr_buffer = buffers.get(idx);
            if idx.idx != 0 {
                if curr_buffer.check_flag(Buffer::READ_ONLY){
                    return Err(EditorErr::ReadOnly(bidx));
                }
                if curr_buffer.check_flag(Buffer::DIRTY) && force == false{
                    return Err(EditorErr::Dirty(bidx));
                }else{
                    buffers.get_mut(idx).clear_flag(Buffer::DIRTY);
                    if curr_view.buf == Some(idx){
                        curr_view.buf = Some(BufferIdx{idx: 0, generation: 0});
                        cmd_line.input.clear();
                        curr_view.off = 0;
                        curr_view.x = 0;
                        curr_view.y = 0;
                        curr_view.prefered_x = 0;
                    }
                    buffers.remove(&mut idx);
                    curr_view.redraw()?;
                }
            }else{
                return Err(EditorErr::Msg("will not close special buffer: 0".into()));
            }
            cmd_line.cursor = 0;
            *mode = Mode::Normal;
            Ok(())
        }
        Cmd::Quit(force)=>{
            if !force{
                let dirty: Vec<_> = buffers.iter()
                    .enumerate()
                    .filter(|(i, b)| b.check_flag(Buffer::DIRTY) && *i != 0)//buffer 0 is special
                    .map(|(i, _)| i)
                    .collect();
                if !dirty.is_empty(){
                    return Err(EditorErr::Msg(format!("cant quit dirty buffers: {:?}",dirty)));
                }
            }
            exit(1);
        },
        Cmd::SwitchBuffer(idx)=>{
            if idx.idx < buffers.len(){
                if buffers.get(idx).check_flag(Buffer::NON_NAVIGATABLE){
                    return Err(EditorErr::Msg(format!("buffer {} is non navigatable",idx.idx)))?;
                }
                let buffer = buffers.get_mut(idx);
                if buffer.buf.len_chars() == 0{
                    if let Some(p) = &buffer.file{
                        let file = File::open(p)?;
                        let reader = BufReader::new(file);
                        buffer.buf = Rope::from_reader(reader)?;
                    }
                }
                curr_view.buf = Some(idx);
                curr_view.y = 0;
                curr_view.x = 0;
                curr_view.off = 0;
                curr_view.prefered_x = 0;
                cmd_line.input.clear();
                cmd_line.cursor = 0;
                *mode = Mode::Normal;
                curr_view.redraw()?;
            }else{
                return Err(EditorErr::InvalidBuffer);
            }
            Ok(())
        }
        Cmd::Open(file)=>{
            curr_view.redraw()?;
            let buffer = if let Some(f) = file{
                if let Some(b) = buffers.get_by_path(&f){
                    *b
                }else{
                    buffers.push(Buffer::new(Some(&f), 0)?)
                }
            }else{
                buffers.push(Buffer::new(None, 0)?)
            };
            curr_view.off = 0;
            curr_view.x = 0;
            curr_view.y = 0;
            curr_view.prefered_x = 0;
            curr_view.buf = Some(buffer);
            cmd_line.input.clear();
            cmd_line.cursor = 0;
            *mode = Mode::Normal;
            Ok(())
        }
        Cmd::NoOp=> Ok(()),
        _ => Ok(()),
    }
}

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

fn parse_cmd(s: String)->Result<Cmd, EditorErr>{
    let s = s.trim();
    let mut parts = s.splitn(2, ' ');
    let cmd = parts.next().ok_or(EditorErr::Msg(format!("unknown command: {}",s)))?;
    let rest = parts.next().unwrap_or("");
    match cmd{
        "q"  => Ok(Cmd::Quit(false)),
        "q!" => Ok(Cmd::Quit(true)),
        "w"  =>{
            let args = parse_args(rest);
            Ok(Cmd::Save(args.get(0).cloned()))
        }
        "open" | "o" => {
            let args = parse_args(rest);
            if let Some(arg) = args.get(0){
                if let Ok(idx) = arg.parse::<usize>(){
                    Ok(Cmd::SwitchBuffer(BufferIdx { idx, generation: 0}))
                }else{
                    Ok(Cmd::Open(Some(arg.clone())))
                }
            }else{
                Ok(Cmd::Open(None))
            }
        }
        "close" | "c" => {
            let mut args = Vec::new();
            args.push(rest);
            if let Some(arg) = args.get(0){
                if let Ok(idx) = arg.parse::<usize>(){
                    Ok(Cmd::Close(Some(BufferIdx {idx, generation:0}), false))
                }else{
                    Ok(Cmd::Close(None, false))
                }
            }else{
                Ok(Cmd::Close(None, false))
            }
        } 
        "close!"| "c!"=> {
            let mut args = Vec::new();
            args.push(rest);
            if let Some(arg) = args.get(0){
                if let Ok(idx) = arg.parse::<usize>(){
                    Ok(Cmd::Close(Some(BufferIdx { idx, generation: 0}), true))
                }else{
                    Ok(Cmd::Close(None, true))
                }
            }else{
                Ok(Cmd::Close(None, true))
            }
        }
        _ => Err(EditorErr::Msg(format!("unknown command: {}",cmd))),
    }
}

fn main()->io::Result<()>{
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut groups = Groups::new();
    let (width, height) = termion::terminal_size().unwrap();
    let mut cmd_line = CmdLine::new(height);
    let height = height -2;
    let mut mode = Mode::Normal;
    {
        let args: Vec<String> = env::args().skip(1).collect();
        let bidx = buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
        let pidx = views.push(View::new(Some(bidx),0,0,width,height,0));
        groups.push(Group::new(&mut views, pidx, View::LINE_NUMBER | View::STATUS_BAR));
        if !args.is_empty(){
            let bidx = buffers.push(Buffer::new(Some(&args[0]), 0).unwrap());
            let pidx = views.push(View::new(Some(bidx), 0, 0, width, height, 0));
            groups.push(Group::new(&mut views, pidx, View::LINE_NUMBER | View::STATUS_BAR));
        }
    }
    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::clear::All)?;

    //inital draw
    cmd_line.draw(mode)?;
    groups.get(GroupIdx{idx:groups.len().saturating_sub(1), generation: 0}).draw_group(mode, &views, &buffers)?;

    for key in input.keys(){
        let cmd = key_to_cmd(key?, &mode);
        let view = groups.get(GroupIdx{idx:groups.len().saturating_sub(1), generation:0}).parent;
        match exec_cmd(&mut cmd_line, view, &mut views, &mut buffers, &mut groups, cmd, &mut mode){
            Err(EditorErr::Msg(msg))=>cmd_line.draw_error(&mut mode, &msg)?,
            Err(EditorErr::Dirty(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is dirty",idx.idx))?,
            Err(EditorErr::InvalidBuffer)=>cmd_line.draw_error(&mut mode, "index is invalid")?,
            Err(EditorErr::ReadOnly(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is read only",idx.idx))?,
            Err(EditorErr::Io(_))=>exit(1),
            Ok(_) => {},
        }
        let group = groups.get(GroupIdx{idx:groups.len().saturating_sub(1),generation:0});

        match mode{
            Mode::Normal | Mode::Insert => {
                group.sync(&mut views);
                group.draw_group(mode, &views, &buffers)?;
            }
            Mode::Command =>{
                group.draw_group(mode, &views, &buffers)?;
                cmd_line.draw(mode)?
            } 
        }
    }
    Ok(())
}
