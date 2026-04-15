use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::stdout;
use std::path::Path;
use crossterm::cursor;
use crossterm::cursor::MoveTo;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use crossterm::execute;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use crossterm::event::{read, Event, KeyCode};
use ropey::Rope;
use std::path::PathBuf;
use std::{env, io};
use std::io::{BufReader, Write};

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
    Log(String),
    Quit,
}

struct Logger{
    file: File,
}
impl Logger{
    fn new(path: &str) -> Self{
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("failed to open log file");
        Self{file}
    }
    fn log(&mut self, msg: &str){
        let msg = format!("{msg}\n");
        self.file.write_all(msg.as_bytes()).unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferIdx{
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
        let path = buf.file.clone();
        let idx = if self.free.is_empty(){
            let idx = BufferIdx{idx:self.data.len(), generation:0};
            self.data.push(buf);
            idx
        }else{
            let mut idx = self.free.pop_front().unwrap();
            idx.generation += 1;
            let element = self.get_mut(idx);
            *element = buf;
            idx
        };
        if let Some(p) = path{
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewIdx{
    idx:usize,
    generation:u64,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GroupIdx{
    idx:usize,
    generation:u64,
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

enum Edit{
    Insert{
        idx:usize,
        text:String,
    },
    Delete{
        idx:usize,
        text:String,
    },
}
struct Buffer{
    generation: u64,
    flags: u64,
    file: Option<PathBuf>,
    buf: Rope,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl Buffer{
    const READ_ONLY:       u64 = 1 << 0;
    const SCRATCH:         u64 = 1 << 1;
    const NEW_FILE:        u64 = 1 << 2;
    const NON_NAVIGATABLE: u64 = 1 << 3;
    fn partial_reset(&mut self){
        self.buf = Rope::new();
        self.undo = Vec::new();
        self.redo = Vec::new();
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
            redo: Vec::new(),
            undo: Vec::new(),
        })
    }
    fn insert(&mut self, view: &View, c: char){
        self.buf.insert_char(view.cursor, c);
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
        queue!(out, MoveTo(1, self.pos_y), Print(s))?;
        Ok(())
    }
    fn draw(&self, mode: Mode)->io::Result<()>{
        let mut out = io::stdout().lock();
        match mode{
            Mode::Command =>{
                queue!(out, MoveTo(0, self.pos_y), Clear(ClearType::CurrentLine))?;
                let s = format!(":{}",self.input);
                queue!(out, MoveTo(0, self.pos_y), Print(s))?;
            }
            Mode::Normal | Mode::Insert => queue!(out, MoveTo(0, self.pos_y), Clear(ClearType::CurrentLine))?,
        }
        Ok(())
    }
}

#[derive(Clone)]
struct View{
    buf: Option<BufferIdx>,
    dirty: Vec<usize>,
    cursor: usize,
    prefered_x: usize,
    off: usize,
    width: u16,
    height: u16,
    pos_x: u16,
    pos_y: u16,
    kind: ViewKind,
}

#[derive(Clone)]
enum ViewKind{
    Text           = 1 << 0,
    LineNumber     = 1 << 1,
    StatusBar      = 1 << 2,
}
impl View{
    fn new(buf: Option<BufferIdx>, kind: ViewKind)->Self{
        Self{
            buf,
            cursor: 0,
            prefered_x: 0,
            off: 0,
            pos_x: 0,
            pos_y: 0,
            width: 0,
            height: 0,
            dirty: vec![],
            kind,
        }
    }
    fn draw_status_bar(&self, idx: BufferIdx, buffers: &Buffers, mode: Mode)->io::Result<()>{
        let buffer = buffers.get(idx);
        let mut out = io::stdout().lock();
        queue!(out, MoveTo(self.pos_x, self.pos_y))?;
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
        let s = format!(" {mode_str} {} {path}",idx.idx);
        let s = format!("{:<width$}", s, width = self.width as usize);
        queue!(out, Print(s))?;
        Ok(())
    }
    fn draw_line_numbers(&self, views: &Views, parent_vidx: ViewIdx) -> io::Result<()> {
        let mut out = io::stdout().lock();

        if views.get(parent_vidx).dirty.is_empty(){
            return Ok(());
        }
        let start = self.off;
        let height = self.height as usize;
        let width = self.width as usize;
        for row in 0..height+1{
            let screen_y = self.pos_y + row as u16;
            let line_num = start + row;

            let s = format!("{:>width$} ", line_num, width = width.saturating_sub(1));
            queue!(out, MoveTo(self.pos_x, screen_y), Print(s))?;
        }
        Ok(())
    }
    fn draw_text(&mut self, buffers: &Buffers) -> io::Result<()>{
        let buffer = if let Some(b) = self.buf{
            b
        }else{
            SCRATCH
        };
        self.dirty.sort_unstable();
        self.dirty.dedup();
        let buffer = buffers.get(buffer);
        let mut out = io::stdout().lock();
        let start = self.off;

        for row in self.dirty.iter(){
            queue!(out, MoveTo(self.pos_x, self.pos_y + *row as u16))?;
            let line_index = start + row;
            if let Some(line) = buffer.buf.get_line(line_index){
                let end = usize::min(self.width as usize, line.len_chars());
                let slice = line.slice(..end.saturating_sub(1));//off by one if not -1 totally didnt spend 2 days trying to find it
                queue!(out, Print(slice))?;
                let remaining = self.width as usize - slice.len_chars();
                for _ in 0..remaining{
                    queue!(out, Print(" "))?;
                }
            }else{
                for _ in 0..self.width{
                    queue!(out, Print(" "))?;
                }
            }
        }
        self.dirty.clear();
        let line = buffer.buf.char_to_line(self.cursor);
        let screen_y = line.saturating_sub(self.off) + self.pos_y as usize;
        let line_start = buffer.buf.line_to_char(line);
        let col = self.cursor - line_start;
        queue!(out, MoveTo(col as u16 + self.pos_x, screen_y as u16))?;
        Ok(())
    }
    fn scroll(&mut self, buffer: &mut Buffer){
        let line = buffer.buf.char_to_line(self.cursor);
        let line_start = buffer.buf.line_to_char(line);
        if line < self.off{
            self.off = line;
            self.dirty.extend(0..self.height as usize);
        } else if line > self.off + self.height as usize{
            self.off = line - self.height as usize;
            self.dirty.extend(0..self.height as usize +1);
        }
        let mut col = self.cursor - line_start;
        if let Some(line) = buffer.buf.get_line(line){
            if line.len_chars() > 0 {
                col = col.min(line.len_chars().saturating_sub(1));
            }else{
                col = 0;
            }
        }else{
            col = 0;
        }
        self.cursor = line_start + col;
    }
}

struct Group{
    generation: u64,
    parent: ViewIdx,
    children: Vec<ViewIdx>,
}
impl Group{
    fn new(views: &mut Views, parent: ViewIdx, flags: &[ViewKind])->Self{
        let mut children = vec![];
        for child in flags{
            match child{
                ViewKind::StatusBar=>{
                    let view = views.push(View::new(None, ViewKind::StatusBar));
                    children.push(view);
                },
                ViewKind::LineNumber=>{
                    let view = views.push(View::new(None, ViewKind::LineNumber));
                    children.push(view);
                },
                _ => {},
            }
        }
        Self{
            generation: 0,
            parent,
            children,
        }
    }
    fn sync(&self, views: &mut Views){
        let (y, off) = {
            let parent = &views.get(self.parent);
            (parent.cursor, parent.off)
        };
        for &child in &self.children{
            let child = views.get_mut(child);
            match child.kind{
                ViewKind::LineNumber=>{
                    child.cursor = y;
                    child.off = off;
                }
                _ => {},
            }
        }
    }
    fn draw_group(&self, mode: Mode, views: &mut Views, buffers: &Buffers)->io::Result<()>{
        for c in self.children.iter(){
            let curr = views.get(*c);
            match curr.kind{
                ViewKind::StatusBar=>{
                    let parent = views.get(self.parent);
                    if let Some(b) = parent.buf{
                        curr.draw_status_bar(b, buffers, mode)?;
                    }
                },
                ViewKind::LineNumber=>{
                    curr.draw_line_numbers(views, self.parent)?;
                },
                ViewKind::Text=>{
                    let curr = views.get_mut(*c);
                    curr.draw_text(buffers)?;
                }
            }
        }
        views.get_mut(self.parent).draw_text(buffers)?;
        Ok(())
    }
    fn resize(&mut self, mut height: u16, mut width: u16, mut pos_x: u16, pos_y: u16, views: &mut Views){
        for c in &self.children{
            let c = views.get_mut(*c);
            match c.kind{
                ViewKind::StatusBar=>{
                    c.pos_x = pos_x;
                    c.pos_y = height + pos_y;
                    c.width = width;
                    c.height = 1;
                    height -= 1;
                },
                ViewKind::LineNumber=>{
                    c.pos_x = pos_x;
                    c.pos_y = pos_y;
                    c.width = 5;
                    c.height = height;
                    pos_x += 5;
                    width -= 5;
                }
                _ =>{},
            }
        }
        let p = views.get_mut(self.parent);
        p.pos_x = pos_x;
        p.pos_y = pos_y;
        p.width = width;
        p.height = height;
        p.dirty.extend(0..p.height as usize+1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodeIdx{
    idx:usize
}
struct Nodes{
    data:Vec<Node>,
    free:Vec<NodeIdx>
}
impl Nodes{
    fn get(&self, idx: NodeIdx)->&Node{
        &self.data[idx.idx]
    }
    fn get_mut(&mut self, idx:NodeIdx)->&mut Node{
        &mut self.data[idx.idx]
    }
    fn push(&mut self, node: Node)->NodeIdx{
        if self.free.is_empty(){
            self.data.push(node);
            NodeIdx {idx: self.data.len().saturating_sub(1)}
        }else{
            let idx = self.free.last_mut().unwrap();
            self.data[idx.idx] = node;
            *idx
        }
    }
    fn remove(&mut self, idx:NodeIdx){
        self.free.push(idx);
    }
    fn len(&self)->usize{
        self.data.len()
    }
}
#[derive(Clone)]
enum Direction{
    Horizontal,
    Vertical,
}
#[derive(Clone)]
enum Node{
    Branch{
        parent: Option<NodeIdx>,
        direction: Direction,
        children: Vec<NodeIdx>,
        focus: usize,
        pos_x:  u16,
        pos_y:  u16,
        width:  u16,
        height: u16,
    },
    Leaf{
        parent: Option<NodeIdx>,
        gidx:GroupIdx,
        height: u16,
        width: u16,
        pos_x: u16,
        pos_y: u16,
    }
}

fn draw(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views, buffers: &Buffers, groups: &mut Groups, mode: Mode)->Result<(), EditorErr>{
    match nodes.get(nidx).clone(){
        Node::Leaf{gidx, ..}=>{
            groups.get(gidx).sync(views);
            groups.get(gidx).draw_group(mode, views, buffers)?;
        },
        Node::Branch{children, focus: f, ..}=>{
                for (idx, c) in children.iter().enumerate(){
                    if idx != f{
                        draw(*c, nodes, views, buffers, groups, mode)?;
                    }
                }
                draw(*children.get(f).unwrap(), nodes, views, buffers, groups, mode)?;
            }
        }
    Ok(())
}

impl Node{
    fn recalc(nidx: NodeIdx, nodes: &Nodes, views: &mut Views, groups: &mut Groups, new_height:Option<u16>, new_width:Option<u16>){
        if let Node::Branch{ direction, pos_x, pos_y, width: w, height: h, children, ..} = nodes.get(nidx){
            let h = if let Some(h2) = new_height{
                h2
            }else{
                *h
            };
            let w = if let Some(w2) = new_width{
                w2
            }else{
                *w
            };
            let mut remainder = {
                match direction {
                    Direction::Vertical=>h/children.len()as u16%h,
                    Direction::Horizontal=>w/children.len()as u16%w,
                }
            };
            let height = {
                match direction{
                    Direction::Horizontal=>h as usize/children.len(),
                    Direction::Vertical=>h as usize,
                }
            };
            let width = {
                match direction {
                    Direction::Vertical=>w as usize/children.len(),
                    Direction::Horizontal=>w as usize,
                }
            };
            let mut pos_x = *pos_x;
            let mut pos_y = *pos_y;
            for c in children{
                let c = nodes.get(*c);
                match c{
                    Node::Leaf{gidx, ..}=>{
                        match direction{
                            Direction::Horizontal=>{
                                if remainder > 0{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16, pos_x, pos_y, views);
                                    remainder -= 1;
                                    pos_y += height as u16+1;
                                }else{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16, pos_x, pos_y, views);
                                    pos_y += height as u16;
                                }
                            },
                            Direction::Vertical=>{
                                if remainder > 0{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16+1, pos_x, pos_y, views);
                                    remainder -= 1;
                                }else{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16, pos_x, pos_y, views);
                                }
                                pos_x += width as u16;
                            }
                        }
                    }
                    _ =>{},
                }
            }
        }
    }
    fn add_leaf(container: NodeIdx, nodes: &mut Nodes, views: &mut Views, groups: &mut Groups, gidx: GroupIdx)->NodeIdx{
        let new = nodes.push(Node::Leaf{gidx, parent:Some(container), height: 0, width: 0, pos_x: 0, pos_y: 0});
        if let Node::Branch {children, ..} = nodes.get_mut(container){
            children.push(new);
        }
        Node::recalc(container, nodes, views, groups, None, None);
        new
    }
    fn leaf_to_container(nidx: NodeIdx, nodes: &mut Nodes, groups: &mut Groups, direction: Direction){
        if let Node::Leaf {gidx, parent, pos_x, pos_y, width, height} = nodes.get(nidx){
            let Group {..} = groups.get(*gidx);
            *nodes.get_mut(nidx) = Node::Branch{parent: *parent, direction, children: vec![], focus:0, pos_x: *pos_x, pos_y:*pos_y, width:*width, height:*height};
        }
    }
    fn remove(remove: NodeIdx, parent: NodeIdx, nodes: &mut Nodes){
        if let Node::Branch {children, ..} = nodes.get_mut(parent){
            children.retain(|x| *x != remove);
            nodes.remove(remove);
        }
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
    CmdExec,
    CmdMoveRight,
    InsertChar(char),
    NewLine,
    Backspace,
    Undo,
    Redo,
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
    Split,
    Vsplit,
    Hsplit,
    FocusUp,
    FocusDown,
    FocusRight,
    FocusLeft,
    ViewClose,
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

fn key_to_cmd(key: KeyEvent, mode: &Mode) -> Cmd {
    if key.code == KeyCode::Esc{
        return Cmd::EnterModeNormal;
    }
    match mode {
        Mode::Normal => {
            match key.code{
                KeyCode::Char('i') => Cmd::EnterModeInsert,
                KeyCode::Char(':') => Cmd::EnterModeCommand,
                KeyCode::Char('u') => Cmd::Undo,
                KeyCode::Char('U') => Cmd::Redo,
                KeyCode::Char('h') | KeyCode::Left => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        Cmd::FocusLeft
                    } else {
                        Cmd::MoveLeft
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        Cmd::FocusDown
                    } else {
                        Cmd::MoveDown
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        Cmd::FocusUp
                    } else {
                        Cmd::MoveUp
                    }
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        Cmd::FocusRight
                    } else {
                        Cmd::MoveRight
                    }
                }
                _ => Cmd::NoOp,
            }
        }
        Mode::Insert => match key.code {
            KeyCode::Up => Cmd::MoveUp,
            KeyCode::Down => Cmd::MoveDown,
            KeyCode::Left => Cmd::MoveLeft,
            KeyCode::Right => Cmd::MoveRight,
            KeyCode::Enter => Cmd::NewLine,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                Cmd::InsertChar(c)
            }
            KeyCode::Backspace => Cmd::Backspace,
            _ => Cmd::NoOp,
        },
        Mode::Command => match key.code {
            KeyCode::Left => Cmd::CmdMoveLeft,
            KeyCode::Right => Cmd::CmdMoveRight,
            KeyCode::Backspace => Cmd::CmdBackspace,
            KeyCode::Enter => Cmd::CmdExec,
            KeyCode::Char(c) => Cmd::CmdInsert(c),
            _ => Cmd::NoOp,
        },
    }
}

fn exec_cmd(nodes: &mut Nodes, focus: &mut NodeIdx, cmd_line: &mut CmdLine, views: &mut Views, buffers: &mut Buffers, groups: &mut Groups, cmd: Cmd, mode: &mut Mode)->Result<(), EditorErr>{
    fn enter_normal(cmd_line: &mut CmdLine, mode: &mut Mode){
        queue!(stdout(), cursor::SetCursorStyle::SteadyBlock).unwrap();
        cmd_line.cursor = 0;
        *mode = Mode::Normal;
    }
    fn get_parent_idx(nodes: &Nodes, nidx: &NodeIdx)->Option<NodeIdx>{
        match nodes.get(*nidx){
            Node::Leaf { parent, ..}=>{
                *parent
            },
            Node::Branch {parent, ..}=>{
                *parent
            },
        }
    }
    fn focus_next(nodes: &mut Nodes, focus: &mut NodeIdx){
        let parent = get_parent_idx(nodes, focus);
        if let Some(p) = parent{
            if let Node::Branch {children, focus: f, .. } = nodes.get_mut(p){
                *f = (*f+1)%children.len();
                *focus = *children.get(*f).unwrap();
            }
        }
    }
    fn focus_prev(nodes: &mut Nodes, focus: &mut NodeIdx){
        let parent = get_parent_idx(nodes, focus);
        if let Some(p) = parent{
            if let Node::Branch {children, focus:f, ..} = nodes.get_mut(p){
                *f = ((*f+children.len())-1)%children.len();
                *focus = *children.get(*f).unwrap();
            }
        }
    }
    fn focus_next_parent(nodes: &mut Nodes, focus: &mut NodeIdx){
        let parent = get_parent_idx(nodes, focus);
        if let Some(p) = parent{
            let parent = get_parent_idx(nodes, &p);
            if let Some(p) = parent{
                if let Node::Branch {children, focus: f, ..} = nodes.get_mut(p){
                    *f = (*f+1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
            }
        }
    }
    fn focus_prev_parent(nodes: &mut Nodes, focus: &mut NodeIdx){
        let parent = get_parent_idx(nodes, focus);
        if let Some(p) = parent{
            let parent = get_parent_idx(nodes, &p);
            if let Some(p) = parent{
                if let Node::Branch {children, focus: f, ..} = nodes.get_mut(p){
                    *f = ((*f+children.len())-1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
            }
        }
    }

    loop{
        if let Node::Branch {focus: f, children, ..} = nodes.get(*focus){
            *focus = *children.get(*f).expect("focus in container is invalid");
        }else{
            break;
        }
    }
    let group = {
        match nodes.get_mut(*focus) {
            Node::Leaf{gidx, ..}=>gidx,
            _ => return Err(EditorErr::Msg("focus cannot be container".to_string()))
        }
    };
    let view = groups.get(*group).parent;
    let bidx = if let Some(b) = views.get(view).buf{
        b
    }else{
        return Err(EditorErr::Msg(format!("invalid buffer: {:?}",views.get(view).buf)));//shouldnt happen
    };
    let buffer = buffers.get_mut(bidx);
    let mut curr_view = views.get_mut(view);
    match cmd{
        Cmd::EnterModeInsert => {
            queue!(stdout(), cursor::SetCursorStyle::SteadyBar)?;
            *mode = Mode::Insert;
            cmd_line.input.clear();
            cmd_line.draw(*mode)?;
            cmd_line.cursor = 0;
            Ok(())
        }
        Cmd::EnterModeNormal => {
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::EnterModeCommand => {
            queue!(stdout(), cursor::SetCursorStyle::SteadyBar).unwrap();
            *mode = Mode::Command;
            cmd_line.input.clear();
            cmd_line.draw(*mode)?;
            Ok(())
        }
        Cmd::InsertChar(c)=>{
            buffer.redo.clear();
            let idx = curr_view.cursor;
            if let Some(edit) = buffer.undo.last_mut(){
                match edit{
                    Edit::Insert {idx: c_idx, text, ..}=>{
                        if *c_idx <= idx && idx <= *c_idx+text.chars().count(){
                            let byte_idx = text.char_indices()
                                .nth(idx - *c_idx)
                                .map(|(b_idx, _)| b_idx)
                                .unwrap_or(text.len());
                            text.insert_str(byte_idx, &c.to_string());
                        }else{
                            buffer.undo.push(Edit::Insert{ idx, text: c.into() });
                        }
                    }
                    Edit::Delete {..}=>{
                        buffer.undo.push(Edit::Insert{ idx, text: c.into() });
                    }
                }
            }else{
                buffer.undo.push(Edit::Insert { idx, text: c.into()});
            }
            buffer.insert(views.get(view), c);
            let view = views.get_mut(view);
            view.cursor += 1;
            let y = buffer.buf.char_to_line(view.cursor);
            view.prefered_x = view.cursor;
            view.dirty.push(y-view.off);
            Ok(())
        },
        Cmd::NewLine=>{
            buffer.redo.clear();
            buffer.insert(&mut curr_view, '\n');
            buffer.undo.push(Edit::Insert{ idx: curr_view.cursor, text: "\n".to_string()});
            let line = buffer.buf.char_to_line(curr_view.cursor);
            curr_view.dirty.extend(line-curr_view.off..curr_view.height as usize);

            let line_start = buffer.buf.line_to_char(line); 
            let col = curr_view.cursor - line_start;
            let line = line + 1;
            let line_start = buffer.buf.line_to_char(line);
            let line_len = buffer.buf.line(line).len_chars();
            curr_view.cursor = line_start + col.min(line_len.saturating_sub(1));
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::Backspace=>{
            buffer.redo.clear();
            if curr_view.cursor != 0{
                let del = buffer.buf.slice(curr_view.cursor-1..curr_view.cursor).to_string();
                if let Some(edit) = buffer.undo.last_mut(){
                    match edit{
                        Edit::Insert {..}=>{
                            buffer.undo.push(Edit::Delete { idx:curr_view.cursor-1, text: del });
                        },
                        Edit::Delete { idx: xidx, text, .. }=>{
                            if *xidx == curr_view.cursor{
                                *xidx -= 1;
                                text.insert_str(0, &del);
                            }else{
                                buffer.undo.push(Edit::Delete { idx: curr_view.cursor - 1, text: del});
                            }
                        }
                    }
                }else{
                    buffer.undo.push(Edit::Delete { idx: curr_view.cursor-1, text: del});
                }
                buffer.buf.remove(curr_view.cursor- 1..curr_view.cursor);
                let line = buffer.buf.char_to_line(curr_view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = curr_view.cursor - line_start;
                if col > line_start{
                    let col = col - 1;
                    curr_view.prefered_x = col;
                    curr_view.dirty.push(line-curr_view.off);
                }else{
                    let line = line.saturating_sub(1);
                    curr_view.dirty.extend(line..curr_view.off+curr_view.height as usize);
                }
                View::scroll(&mut curr_view, buffer);
            }
            Ok(())
        },
        Cmd::Undo=>{
            if let Some(edit) = buffer.undo.pop(){
                match edit{
                    Edit::Insert { idx, text, }=>{
                        buffer.redo.push(Edit::Delete { idx, text: text.clone() });
                        buffer.buf.remove(idx..idx + text.chars().count());
                        curr_view.cursor = idx;
                    },
                    Edit::Delete { idx, text, }=>{
                        buffer.redo.push(Edit::Insert {idx, text: text.clone()});
                        buffer.buf.insert(idx, &text);
                        curr_view.cursor = idx;
                    },
                }
                let line = buffer.buf.char_to_line(curr_view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = curr_view.cursor - line_start;
                curr_view.prefered_x = col;
                curr_view.dirty.extend(line-curr_view.off..curr_view.height as usize);
                return Ok(())
            }
            Err(EditorErr::Msg("undo stack is empty".to_string()))
        },
        Cmd::Redo => {
            if let Some(edit) = buffer.redo.pop() {
                match edit {
                    Edit::Insert { idx, text } => {
                        buffer.buf.remove(idx..idx + text.chars().count());
                        curr_view.cursor = idx;
                        buffer.undo.push(Edit::Delete{idx, text});
                    }
                    Edit::Delete { idx, text } => {
                        buffer.buf.insert(idx, &text);
                        curr_view.cursor = idx;
                        buffer.undo.push(Edit::Insert{ idx, text });
                    }
                }
                let line = buffer.buf.char_to_line(curr_view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = curr_view.cursor - line_start;
                curr_view.prefered_x = col;
                curr_view.dirty.extend(line-curr_view.off..curr_view.height as usize);
                View::scroll(curr_view, buffer);
                return Ok(());
            }
            Err(EditorErr::Msg("redo stack is empty".to_string()))
        },
        Cmd::MoveUp=>{
            let line = buffer.buf.char_to_line(curr_view.cursor);
            if line > 0 {
                let line = line - 1;
                let line_start = buffer.buf.line_to_char(line);
                let line_len = buffer.buf.line(line).len_chars();
                let col = curr_view.prefered_x.min(line_len.saturating_sub(1));

                curr_view.cursor = line_start + col;
            }
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::MoveDown=>{
            let len_lines = buffer.buf.len_lines();
            let line = buffer.buf.char_to_line(curr_view.cursor);
            if line + 1 < len_lines{
                let line = line + 1;
                let start = buffer.buf.line_to_char(line);
                let len = buffer.buf.line(line).len_chars();
                let col = curr_view.prefered_x.min(len.saturating_sub(1));
                curr_view.cursor = start + col;
                View::scroll(&mut curr_view, buffer);
            }
            Ok(())
        },
        Cmd::MoveRight=>{
            let line = buffer.buf.char_to_line(curr_view.cursor);
            let line_start = buffer.buf.line_to_char(line);
            let line_len = buffer.buf.line(line).len_chars();
            if curr_view.cursor < line_start + line_len.saturating_sub(1){
                let col = curr_view.cursor - line_start; 
                let col = col + 1;
                let col = col.min(buffer.buf.line(line).len_chars().saturating_sub(1));
                curr_view.prefered_x = col;
                curr_view.cursor = line_start + col;
            }
            Ok(())
        },
        Cmd::MoveLeft=>{
            let line = buffer.buf.char_to_line(curr_view.cursor);
            let line_start = buffer.buf.line_to_char(line);
            let col = curr_view.cursor - line_start;
            if curr_view.cursor > line_start {
                let col = col - 1;
                curr_view.prefered_x = col;
                curr_view.cursor = line_start + col;
            }
            Ok(())
        },
        Cmd::Split=>{
            let view = views.push(View::new(Some(SCRATCH), ViewKind::Text));
            let gidx = groups.push(Group::new(views, view, &[ViewKind::StatusBar, ViewKind::LineNumber]));
            let parent = get_parent_idx(nodes, focus);
            if let Some(p) = parent{
                let view = views.get_mut(view);
                view.dirty.extend(0..view.height as usize);
                Node::add_leaf(p, nodes, views, groups, gidx);
                focus_next(nodes, focus);
                enter_normal(cmd_line, mode);
            }
            Ok(())
        }
        Cmd::Vsplit=>{
            let group = group.clone();
            let view = views.get_mut(groups.get(group).parent); 
            view.dirty.extend(0..view.height as usize);
            Node::leaf_to_container(*focus, nodes, groups, Direction::Vertical);
            Node::add_leaf(*focus, nodes, views, groups, group);
            let parent = views.push(View::new(Some(SCRATCH), ViewKind::Text));
            let gidx = groups.push(Group::new(views, parent, &[ViewKind::StatusBar, ViewKind::LineNumber]));
            *focus = Node::add_leaf(*focus, nodes, views, groups, gidx);
            let view = views.get_mut(groups.get(gidx).parent);
            view.dirty.extend(0..view.height as usize);
            focus_next(nodes, focus);
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::Hsplit=>{
            let group = group.clone();
            Node::leaf_to_container(*focus, nodes, groups, Direction::Horizontal);
            Node::add_leaf(*focus, nodes, views, groups, group);
            let parent = views.push(View::new(Some(SCRATCH), ViewKind::Text));
            let gidx = groups.push(Group::new(views, parent, &[ViewKind::StatusBar, ViewKind::LineNumber]));
            *focus = Node::add_leaf(*focus, nodes, views, groups, gidx);
            let view = views.get_mut(groups.get(gidx).parent);
            view.dirty.extend(0..view.height as usize);
            focus_next(nodes, focus);
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::ViewClose=>{
            let parent_idx = get_parent_idx(nodes, focus);
            let mut remove = *focus;
            let mut parent = parent_idx;
            while remove != ROOT{
                let p = {
                    if let Some(p) = parent{
                        p
                    }else{
                        break;
                    }
                };
                if p == ROOT{
                    if let Node::Branch {children, ..} = nodes.get(p){
                        if children.len() <= 1{
                            Node::recalc(p, nodes, views, groups, None, None);
                            break;
                        }
                    }
                }
                Node::remove(remove, p, nodes);
                if let Node::Branch { parent: p2, children, width:w, height:h, ..} = nodes.get(p){
                    if let Some(p2) = p2{
                        if let Node::Branch {width: w2, height:h2, ..} = nodes.get(*p2){
                            Node::recalc(*p2, nodes, views, groups, Some(w+w2), Some(h+h2));
                        }
                        if children.is_empty(){
                            remove = p;
                            parent = Some(*p2);
                        }else{
                            break;
                        }
                    }else{
                        break;
                    }
                }else{
                    break;
                }
            }
            if let Some(p) = parent{
                if let Node::Branch {children, focus: f, ..} = nodes.get_mut(p){
                    *f = (*f+1)%children.len();
                    *focus = children[*f];
                }
            }
            queue!(stdout(), Clear(ClearType::All))?;
            let parent = get_parent_idx(nodes, focus);//if you forget to recalc one last time scroll stops working properly if there is only one view
            if let Some(p) = parent{
                Node::recalc(p, nodes, views, groups, None, None);
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusDown=>{
            let parent = match get_parent_idx(nodes, focus) {
                Some(p)=>p,
                None=>return Ok(())
            };
            if let Node::Branch { direction, ..} = nodes.get(parent){
                match direction{
                    Direction::Vertical=>focus_next_parent(nodes, focus),
                    Direction::Horizontal=>focus_next(nodes, focus),
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusUp=>{
            let parent = match get_parent_idx(nodes, focus){
                Some(p)=>p,
                None=>return Ok(()),
            };
            if let Node::Branch {direction, ..} = nodes.get(parent){
                match direction{
                    Direction::Vertical=>focus_prev_parent(nodes, focus),
                    Direction::Horizontal=>focus_prev(nodes, focus),
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusRight=>{
            let parent = match get_parent_idx(nodes, focus){
                Some(p)=>p,
                None=>return Ok(()),
            };
            if let Node::Branch {direction, focus: f, children, ..} = nodes.get(parent){
                if *f == children.len(){
                    focus_next_parent(nodes, focus);
                }else{
                    match direction{
                        Direction::Horizontal=>focus_next_parent(nodes, focus),
                        Direction::Vertical=>focus_next(nodes, focus),
                    }
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusLeft=>{
            let parent = match get_parent_idx(nodes, focus) {
                Some(p)=>p,
                None=>return Ok(())
            };
            if let Node::Branch {direction, focus: f, ..} = nodes.get(parent){
                if *f == 0{
                    focus_prev_parent(nodes, focus);
                }else{
                    match direction {
                        Direction::Horizontal=>focus_prev_parent(nodes, focus),
                        Direction::Vertical=>focus_prev(nodes, focus),
                    }
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::CmdExec=>{
            let input = cmd_line.input.clone();
            match parse_cmd(input){
                Ok(parsed_cmd) => exec_cmd(nodes, focus, cmd_line, views, buffers, groups, parsed_cmd, mode)?,
                Err(e) => {
                    cmd_line.draw_error(mode, &format!("{:?}", e))?;
                }
            }
            Ok(())
        }
        Cmd::CmdInsert(c)=>{
            cmd_line.insert(c);
            cmd_line.draw(*mode)?;
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
                buffer.undo.clear();
                buffer.redo.clear();
            }else{
                if let Some(_) = &buffer.file{
                    match buffer.save(None){
                        Err(error)=>return Err(EditorErr::Io(error)),
                        Ok(_)=>{buffer.undo.clear(); buffer.redo.clear();},
                    }
                }else{
                    return Err(EditorErr::Msg("new file needs name".into()));
                }
            }
            enter_normal(cmd_line, mode);
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
            if idx != SCRATCH{
                if curr_buffer.check_flag(Buffer::READ_ONLY){
                    return Err(EditorErr::ReadOnly(bidx));
                }
                if !curr_buffer.undo.is_empty() && force == false{
                    return Err(EditorErr::Dirty(bidx));
                }else{
                    if curr_view.buf == Some(idx){
                        curr_view.buf = Some(SCRATCH);
                        cmd_line.input.clear();
                        curr_view.off = 0;
                        curr_view.cursor = 0;
                        curr_view.prefered_x = 0;
                    }
                    buffers.remove(&mut idx);
                }
                curr_view.dirty.extend(0..curr_view.height as usize+1);
            }else{
                return Err(EditorErr::Msg("will not close special buffer: 0".into()));
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::Quit(force)=>{
            if !force{
                let dirty: Vec<_> = buffers.iter()
                    .enumerate()
                    .filter(|(i, b)| !b.undo.is_empty() && *i != SCRATCH.idx)
                    .map(|(i, _)| i)
                    .collect();
                if !dirty.is_empty(){
                    return Err(EditorErr::Msg(format!("cant quit dirty buffers: {:?}",dirty)));
                }
            }
            Err(EditorErr::Quit)
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
                curr_view.cursor = 0;
                curr_view.off = 0;
                curr_view.prefered_x = 0;
                curr_view.dirty.extend(0..curr_view.height as usize+1);
                enter_normal(cmd_line, mode);
            }else{
                return Err(EditorErr::InvalidBuffer);
            }
            Ok(())
        }
        Cmd::Open(file)=>{
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
            curr_view.cursor = 0;
            curr_view.prefered_x = 0;
            curr_view.dirty.extend(0..curr_view.height as usize);
            curr_view.buf = Some(buffer);
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::NoOp=> Ok(()),
        _ => Ok(()),
    }
}

fn parse_cmd(s: String)->Result<Cmd, EditorErr>{
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
    let cmd = parts.next().ok_or(EditorErr::Msg(format!("unknown command: {}",s)))?;
    let rest = parts.next().unwrap_or("");
    match cmd{
        "q"  => Ok(Cmd::Quit(false)),
        "Q" => Ok(Cmd::Quit(true)),
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
        "split"  | "s"  =>Ok(Cmd::Split),
        "splitv" | "sv" =>Ok(Cmd::Vsplit),
        "splith" | "sh" =>Ok(Cmd::Hsplit),
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
        "CLOSE"| "C" => {
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
        "viewclose" | "vc"=> Ok(Cmd::ViewClose),
        "right"=> Ok(Cmd::FocusRight),
        "left" => Ok(Cmd::FocusLeft),
        "down" => Ok(Cmd::FocusDown),
        "up"   => Ok(Cmd::FocusUp),
        _ => Err(EditorErr::Msg(format!("unknown command: {}",cmd))),
    }
}


const ROOT: NodeIdx = NodeIdx{idx:0};
const SCRATCH: BufferIdx = BufferIdx{idx: 0, generation: 0};
fn main()->io::Result<()>{
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut groups = Groups::new();
    let mut nodes = Nodes{data:vec![], free:vec![]};
    let (width, height) = terminal::size().unwrap();
    let mut cmd_line = CmdLine::new(height);
    let height = height -2;
    let mut mode = Mode::Normal;
    let root = nodes.push(Node::Branch{parent: None, direction: Direction::Vertical, width, height, pos_x:0, pos_y:0, focus:0, children:vec![]});
    let mut logger = Logger::new("log");
    {
        buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
        let args: Vec<String> = env::args().skip(1).collect();
        let bidx = {
            if args.is_empty(){
                SCRATCH
            }else{
                buffers.push(Buffer::new(Some(&args[0]), 0).unwrap())
            }
        };
            let vidx = views.push(View::new(Some(bidx), ViewKind::Text));
            let gidx = groups.push(Group::new(&mut views, vidx, &[ViewKind::StatusBar, ViewKind::LineNumber]));
            Node::add_leaf(root, &mut nodes, &mut views, &mut groups, gidx);
    }
    enable_raw_mode()?;
    execute!(stdout(), Clear(ClearType::All))?;

    //inital draw
    cmd_line.draw(mode)?;
    draw(root, &mut nodes, &mut views, &buffers, &mut groups, mode).unwrap();
    let mut focus = NodeIdx{idx:1};
    stdout().flush().unwrap();

    loop{
        if let Event::Key(event) = read()?{
            let cmd = key_to_cmd(event, &mode);
            match exec_cmd(&mut nodes, &mut focus, &mut cmd_line, &mut views, &mut buffers, &mut groups, cmd, &mut mode){
                Err(EditorErr::Msg(msg))=>cmd_line.draw_error(&mut mode, &msg)?,
                Err(EditorErr::Dirty(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is dirty",idx.idx))?,
                Err(EditorErr::InvalidBuffer)=>cmd_line.draw_error(&mut mode, "index is invalid")?,
                Err(EditorErr::ReadOnly(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is read only",idx.idx))?,
                Err(EditorErr::Log(msg))=>logger.log(&msg),
                Err(EditorErr::Io(_))=>break,
                Err(EditorErr::Quit)=>break,
                Ok(_) => {},
            }
            queue!(stdout(), cursor::Hide)?;
            draw(root, &mut nodes, &mut views, &buffers, &mut groups, mode).unwrap();
            match mode{
                Mode::Command =>{
                    cmd_line.draw(mode)?
                } 
                _ => {},
            }
            queue!(stdout(), cursor::Show)?;
        }
        stdout().flush()?;
    }
    disable_raw_mode().unwrap();
    execute!(stdout(), Clear(ClearType::All)).unwrap();
    Ok(())
}
