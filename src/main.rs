use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::stdout;
use std::path::Path;
use std::sync::Mutex;
use std::usize;
use std::vec;
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
fn log(msg: &str){
    LOGGER.lock().unwrap().log(msg);
}
static LOGGER: Mutex<Logger> = Mutex::new(Logger {
    file: "log",
});

trait ExpectLog<T> {
    fn expect_log(self, msg: &str)->T;
}
impl<T> ExpectLog<T> for Option<T> {
    fn expect_log(self, msg: &str)->T {
        match self{
            Some(v) => v,
            None =>{
                log(msg);
                panic!("{}",msg);
            }
        }
    }
}

impl<T, E: std::fmt::Debug> ExpectLog<T> for Result<T, E> {
    fn expect_log(self, msg: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => {
                log(&format!("{}: {:?}", msg, e));
                panic!("{}: {:?}", msg, e);
            }
        }
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
    fn scroll(&mut self, buffer: &mut Buffer){
        let line = buffer.buf.char_to_line(self.cursor);
        let line_start = buffer.buf.line_to_char(line);
        if line < self.off{
            self.off = line;
            self.dirty.extend(0..self.height as usize+1);
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
}

enum Node{
    Leaf{
        parent: NodeIdx,
        gidx: GroupIdx,
        generation: u16,
    },
    Branch{
        parent: Option<NodeIdx>,
        children: Vec<NodeIdx>,
        direction: Direction,
        focus: usize,
        generation: u16,
        pos_x: u16,
        pos_y: u16,
        width: u16,
        height: u16,
    },
}

#[derive(Clone)]
enum Direction{
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodeIdx{
    idx:usize,
    generation:u16,
}
struct Nodes{
    data:Vec<Node>,
    free:Vec<usize>,
}

impl Nodes{
    fn new_leaf(nodes: &mut Nodes, gidx: GroupIdx, parent: NodeIdx, views: &mut Views, groups: &mut Groups, focus: &mut NodeIdx)->NodeIdx{
        let new = Node::Leaf{ parent, gidx, generation: 0};
        let idx = nodes.push(new);
        if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(parent.idx).expect_log("parent is leaf"){
            children.push(idx);
            *f = (*f+1)%children.len();
            *focus = *children.get(*f).expect_log("invalid focus 483");
            while let Node::Branch {children, focus: f, ..} = nodes.data.get(focus.idx).expect_log("idk"){
                *focus = *children.get(*f).expect_log("children.get is none");
            }
        }
        reflow(parent, nodes, views, groups, focus);
        idx
    }
    //need to resize and then add children imediately and manually and call reflow on parent
    fn new_branch(nodes: &mut Nodes, parent: NodeIdx, views: &mut Views, groups: &mut Groups, reserved: NodeIdx, direction: Direction, focus: &mut NodeIdx)->NodeIdx{
        let new = Node::Branch { parent: Some(parent), direction, generation: 0, focus: 0, pos_x: 0, pos_y: 0, width: 0, height: 0, children: vec![reserved]};
        let idx = nodes.push(new);
        if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(parent.idx).expect("parent is leaf"){
            children.push(idx);
            *f = children.len().saturating_sub(1);
            *focus = *children.get(*f).unwrap();
            while let Node::Branch {children, focus: f, ..} = nodes.data.get(focus.idx).unwrap(){
                *focus = *children.get(*f).unwrap();
            }
            log(&format!("focus:{}",focus.idx));
        }
        reflow(parent, nodes, views, groups, focus);
        idx
    }
    fn push(&mut self, new: Node)->NodeIdx{
        let generation = {
            match new{
                Node::Leaf {generation, ..}=>generation,
                Node::Branch {generation, ..}=>generation,
            }
        };
        if self.free.is_empty(){
            self.data.push(new);
            NodeIdx{ idx:self.data.len().saturating_sub(1), generation: 0 }
        }else{
            let idx = self.free.last_mut().unwrap();
            let old = self.data.get_mut(*idx).unwrap();
            *old = new;
            NodeIdx { idx: *idx, generation}
        }
    }
    fn reserve(&mut self)->NodeIdx{
        if self.free.is_empty(){
            self.data.push(Node::Leaf { parent: NodeIdx {idx: 0, generation: 0}, gidx: GroupIdx { idx: 0, generation: 0 }, generation:0});
            NodeIdx{idx:self.data.len().saturating_sub(1), generation: 0}
        }else{
            let idx = *self.free.last_mut().unwrap();
            match self.data.get_mut(idx).unwrap(){
                Node::Leaf {generation, ..}=>*generation = 0,
                Node::Branch {generation, ..}=>*generation = 0,
            }
            NodeIdx { idx, generation:0}
        }
    }
    fn remove(&mut self, nidx: NodeIdx){
        match self.data.get_mut(nidx.idx).unwrap(){
            Node::Leaf {generation, ..}=>*generation += 1,
            Node::Branch {generation, ..}=>*generation += 1,
        }
        self.free.push(nidx.idx);
    }
}

const ROOT: NodeIdx = NodeIdx{idx: 0, generation: 0};
fn paint(nidx: NodeIdx, mode: &Mode, nodes: &Nodes, views: &mut Views, groups: &Groups, buffers: &Buffers)->io::Result<()>{
    let mut out = io::stdout().lock();
    match nodes.data.get(nidx.idx).unwrap(){
        Node::Leaf {gidx, ..}=>{
            let g = groups.get(*gidx);
            let p = views.get(g.parent);
            let bidx = p.buf.unwrap_or(SCRATCH);
            let buffer = buffers.get(p.buf.unwrap());
            for c in &g.children{
                let c = views.get(*c);
                match c.kind{
                    ViewKind::LineNumber=>{
                        if p.dirty.is_empty(){continue;}
                        for row in 0..c.height+1{
                            let screen_y = p.pos_y + row as u16;
                            let line_num = p.off + row as usize;
                            let s = format!("{:>width$}", line_num, width = (c.width - 1)as usize);
                            queue!(out, MoveTo(c.pos_x, screen_y), Print(s))?;
                        }
                    }
                    ViewKind::StatusBar=>{
                        if p.dirty.is_empty(){continue;}
                        let mut path = "SCRATCH";
                        if !buffer.check_flag(Buffer::SCRATCH){
                            if let Some(p) = &buffer.file{
                                path = p.to_str().unwrap_or("NEW_FILE");
                            }else{
                                path = "NEW_FILE";
                            }
                        }
                        let mode_str = match mode{
                            Mode::Command=> "CMD",
                            Mode::Insert => "INS",
                            _ => "NOR",
                        };
                        let s = format!("{mode_str} {} {path}", bidx.idx);
                        let s = format!("{:<width$}", s, width = c.width as usize);
                        queue!(out, MoveTo(c.pos_x, c.pos_y), Print(s))?;
                    }
                    _ => {},
                }
            }
            let p = views.get_mut(g.parent);
            p.dirty.sort_unstable();
            p.dirty.dedup();
            for row in p.dirty.iter(){
                queue!(out, MoveTo(p.pos_x, p.pos_y + *row as u16))?;
                let line_idx = p.off + row;
                if let Some(line) = buffer.buf.get_line(line_idx){
                    let end = usize::min(p.width as usize, line.len_chars());
                    let slice = line.slice(..end.saturating_sub(1));
                    queue!(out, Print(slice))?;
                    let rem = p.width as usize - slice.len_chars();
                    for _ in 0..rem{
                        queue!(out, Print(" "))?;
                    }
                }else{
                    for _ in 0..p.width{
                        queue!(out, Print(" "))?;
                    }
                }
            }
            p.dirty.clear();
            let line = buffer.buf.char_to_line(p.cursor);
            let screen_y = line.saturating_sub(p.off) + p.pos_y as usize;
            let line_start = buffer.buf.line_to_char(line);
            let col = p.cursor - line_start;
            queue!(out, MoveTo(col as u16 + p.pos_x, screen_y as u16))?;
        }
        Node::Branch {children, focus, ..}=>{
            for (i, c) in children.iter().enumerate(){
                if i != *focus as usize{
                    paint(*c, mode, nodes, views, groups, buffers)?;
                }
            }
            let idk = children.get(*focus).unwrap();
            paint(*idk, mode, nodes, views, groups, buffers)?;
        }
    }
    Ok(())
}

fn recalc(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views, groups: &mut Groups){
    let Node::Branch{children, direction, pos_x, pos_y, width, height, ..} = nodes.data.get(nidx.idx).unwrap() else {return;};
    let (width, height) = {
        match direction {
            Direction::Horizontal=>(*width, height/children.len() as u16),
            Direction::Vertical=>(width/children.len() as u16, *height),
        }
    };
    let children = children.clone();
    let direction = direction.clone();
    let mut pos_x = pos_x.clone();
    let mut pos_y = pos_y.clone();
    for c in children.iter(){
        let c = nodes.data.get_mut(c.idx).unwrap();
        match c {
            Node::Branch{..}=>{
                branch_recalc(c, height, width, pos_x, pos_y);
            }
            Node::Leaf{gidx, ..}=>{
                group_recalc(&groups.get_mut(*gidx), views, height, width, pos_x, pos_y);
            }
        }
        match direction{
            Direction::Vertical=>pos_x += width,
            Direction::Horizontal=>pos_y += height,
        }
    }
    fn branch_recalc(node: &mut Node, height: u16, width: u16, pos_x: u16, pos_y: u16){
        let Node::Branch {pos_x: x, pos_y: y, width: w, height: h, ..} = node else {return};
        *x = pos_x;
        *y = pos_y;
        *w = width;
        *h = height;
    }
    fn group_recalc(group: &Group, views: &mut Views, height: u16, width: u16, pos_x: u16, pos_y: u16){
        let mut h = height; 
        let mut w = width;
        let mut x = pos_x;
        let mut y = pos_y;
        for c in group.children.iter(){
            let c = views.get_mut(*c);
            match c.kind {
                ViewKind::LineNumber=>{
                    c.pos_x = x;
                    c.pos_y = y;
                    c.height = h;
                    c.width = 5;
                    w -= 5;
                    x += 5;
                }
                ViewKind::StatusBar=>{
                    c.pos_y = h;
                    c.pos_x = x;
                    c.width = w;
                    h -= 1;
                }
                _ => {},
            }
        }
        let p = views.get_mut(group.parent);
        p.width = w;
        p.height = h;
        p.pos_x = x;
        p.pos_y = y;
        p.dirty.extend(0..p.height as usize+1);
    }
}
fn reflow(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views, groups: &mut Groups, focus: &mut NodeIdx){
    let mut rm: Vec<(NodeIdx, usize, NodeIdx)> = vec![]; //parent,child index,nodeidx for child
    let mut to_recalc: Vec<NodeIdx> = vec![];
    mark(nidx, &mut rm, &mut to_recalc, nodes);
    for (p, i, n) in rm.iter().rev(){
        let Node::Branch {focus: f, children, ..} = nodes.data.get_mut(p.idx).unwrap() else {return};
        children.remove(*i);
        *f = (*f+children.len()-1)%children.len();
        *focus = *children.get(*f).unwrap();
        while let Node::Branch {focus:f, children, ..} = nodes.data.get(focus.idx).unwrap(){
            *focus = *children.get(*f).unwrap();
        }
        nodes.remove(*n);
    }
    for c in to_recalc{
        recalc(c, nodes, views, groups);
    }

    fn mark(nidx: NodeIdx, rm: &mut Vec<(NodeIdx, usize, NodeIdx)>, recalc: &mut Vec<NodeIdx>, nodes: &Nodes){
        let node = nodes.data.get(nidx.idx).unwrap();
        let Node::Branch {parent, children, ..} = node else{
            return
        };
        recalc.push(nidx);
        for(c, n) in children.iter().enumerate(){
            let child = nodes.data.get(n.idx).unwrap();
            match child{
                Node::Leaf { generation, .. }=>{
                    if *generation != n.generation{
                        rm.push((nidx, c, *n));
                    }
                },
                Node::Branch {children, generation, ..}=>{
                    if children.is_empty(){
                        rm.push((nidx, c, *n));
                    }else if *generation != n.generation{
                        mark(*n, rm, recalc, nodes);
                        recalc.push(*n);
                    }
                }
            }
        }
        if children.is_empty(){
            if let Some(p)  = parent{
                mark(*p, rm, recalc, nodes);
            }
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

    let get_parent = |nidx: NodeIdx| ->Result<NodeIdx, EditorErr> {
        let Node::Leaf {parent, ..} = nodes.data.get(nidx.idx).unwrap() else {return Err(EditorErr::InvalidFocus)};
        Ok(*parent)
    };

    let gidx = || ->Result<GroupIdx, EditorErr> {
        match nodes.data.get(focus.idx).unwrap(){
            Node::Leaf{gidx, ..}=>Ok(*gidx),
            _ => return Err(EditorErr::Msg("focus cannot be container".to_string()))
        }
    };
    let vidx = || ->Result<(ViewIdx, GroupIdx), EditorErr>{
        let gidx = gidx()?;
        Ok((groups.get(gidx).parent, gidx))
    };
    let bidx = || -> Result<(BufferIdx, ViewIdx, GroupIdx), EditorErr>{
        let (vidx, gidx) = vidx()?;
        if let Some(b) = views.get(vidx).buf{
            Ok((b, vidx, gidx))
        }else{
            Err(EditorErr::InvalidBuffer)
        }
    };
    // let view = groups.get(*group).parent;
    // let bidx = if let Some(b) = views.get(view).buf{
    //     b
    // }else{
    //     return Err(EditorErr::Msg(format!("invalid buffer: {:?}",views.get(view).buf)));//shouldnt happen
    // };
    // let buffer = buffers.get_mut(bidx);
    // let mut curr_view = views.get_mut(view);

    let (bidx, vidx, gidx) = bidx()?;
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
            let view = views.get(vidx);
            let buffer = buffers.get_mut(bidx);
            buffer.redo.clear();
            let view = views.get(vidx);
            if let Some(edit) = buffer.undo.last_mut(){
                match edit{
                    Edit::Insert {idx: c_idx, text, ..}=>{
                        if *c_idx <= view.cursor && view.cursor <= *c_idx+text.chars().count(){
                            let byte_idx = text.char_indices()
                                .nth(view.cursor - *c_idx)
                                .map(|(b_idx, _)| b_idx)
                                .unwrap_or(text.len());
                            text.insert_str(byte_idx, &c.to_string());
                        }else{
                            buffer.undo.push(Edit::Insert{ idx: view.cursor, text: c.into() });
                        }
                    }
                    Edit::Delete {..}=>{
                        buffer.undo.push(Edit::Insert{ idx: view.cursor, text: c.into() });
                    }
                }
            }else{
                buffer.undo.push(Edit::Insert { idx: view.cursor, text: c.into()});
            }
            buffer.insert(view, c);
            let view = views.get_mut(vidx);
            let line = buffer.buf.char_to_line(view.cursor);
            let line_start = buffer.buf.line_to_char(line);
            let col = view.cursor +1 - line_start;

            let line_end = buffer.buf.line(line).len_chars();
            let col = col.min(line_end.saturating_sub(1));

            view.cursor = line_start + col;
            view.prefered_x = view.cursor - line_start;
            view.dirty.push(line-view.off);
            Ok(())
        },
        Cmd::NewLine=>{
            let view = views.get_mut(vidx);
            let buffer = buffers.get_mut(bidx);
            buffer.redo.clear();
            buffer.insert(view, '\n');
            buffer.redo.push(Edit::Insert { idx: view.cursor, text: "\n".to_string() });
            let line = buffer.buf.char_to_line(view.cursor)+1;
            let len_lines = buffer.buf.len_lines();
            let line = line.min(len_lines);
            let line_start = buffer.buf.line_to_char(line);
            view.dirty.extend(line..view.height as usize+1);
            view.cursor = line_start;
            View::scroll(view, buffer);
            Ok(())
        },
        Cmd::Backspace=>{
            let view = views.get_mut(vidx);
            let buffer = buffers.get_mut(bidx);
            buffer.redo.clear();
            if view.cursor != 0{
                let del = buffer.buf.slice(view.cursor-1..view.cursor).to_string();
                if let Some(edit) = buffer.undo.last_mut(){
                    match edit{
                        Edit::Insert {..}=>{
                            buffer.undo.push(Edit::Delete { idx:view.cursor-1, text: del });
                        },
                        Edit::Delete { idx: xidx, text, .. }=>{
                            if *xidx == view.cursor{
                                *xidx -= 1;
                                text.insert_str(0, &del);
                            }else{
                                buffer.undo.push(Edit::Delete { idx: view.cursor - 1, text: del});
                            }
                        }
                    }
                }else{
                    buffer.undo.push(Edit::Delete { idx: view.cursor-1, text: del});
                }
                let line = buffer.buf.char_to_line(view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = view.cursor - line_start;
                let prev_start = buffer.buf.line_to_char(line.saturating_sub(1));
                let prev_len = buffer.buf.line(line.saturating_sub(1)).len_chars().saturating_sub(1);
                buffer.buf.remove(view.cursor- 1..view.cursor);
                if view.cursor > line_start{
                    let col = col - 1;
                    view.prefered_x = col;
                    view.cursor = line_start + col;
                }else{
                    view.prefered_x = prev_len;
                    view.cursor = prev_start + prev_len;
                }
                View::scroll(view, buffer);
            }
            Ok(())
        },
        Cmd::Undo=>{
            let view = views.get_mut(vidx);
            let buffer = buffers.get_mut(bidx);
            if let Some(edit) = buffer.undo.pop(){
                match edit{
                    Edit::Insert { idx, text, }=>{
                        buffer.redo.push(Edit::Delete { idx, text: text.clone() });
                        buffer.buf.remove(idx..idx + text.chars().count());
                        view.cursor = idx;
                    },
                    Edit::Delete { idx, text, }=>{
                        buffer.redo.push(Edit::Insert {idx, text: text.clone()});
                        buffer.buf.insert(idx, &text);
                        view.cursor = idx;
                    },
                }
                let line = buffer.buf.char_to_line(view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = view.cursor - line_start;
                view.prefered_x = col;
                view.dirty.extend(line-view.off..view.height as usize);
                return Ok(())
            }
            Err(EditorErr::Msg("undo stack is empty".to_string()))
        },
        Cmd::Redo => {
            let view = views.get_mut(vidx);
            let buffer = buffers.get_mut(bidx);
            if let Some(edit) = buffer.redo.pop() {
                match edit {
                    Edit::Insert { idx, text } => {
                        buffer.buf.remove(idx..idx + text.chars().count());
                        view.cursor = idx;
                        buffer.undo.push(Edit::Delete{idx, text});
                    }
                    Edit::Delete { idx, text } => {
                        buffer.buf.insert(idx, &text);
                        view.cursor = idx;
                        buffer.undo.push(Edit::Insert{ idx, text });
                    }
                }
                let line = buffer.buf.char_to_line(view.cursor);
                let line_start = buffer.buf.line_to_char(line);
                let col = view.cursor - line_start;
                view.prefered_x = col;
                view.dirty.extend(line-view.off..view.height as usize);
                let view = views.get_mut(vidx);
                let buffer = buffers.get_mut(bidx);
                View::scroll(view, buffer);
                return Ok(());
            }
            Err(EditorErr::Msg("redo stack is empty".to_string()))
        },
        Cmd::MoveUp=>{
            let buffer = buffers.get(bidx);
            let view = views.get_mut(vidx);
            let line = buffer.buf.char_to_line(view.cursor);
            if line > 0 {
                let line = line - 1;
                let line_start = buffer.buf.line_to_char(line);
                let line_len = buffer.buf.line(line).len_chars();
                let col = view.prefered_x.min(line_len.saturating_sub(1));

                view.cursor = line_start + col;
            }
            let buffer = buffers.get_mut(bidx);
            View::scroll(view, buffer);
            Ok(())
        },
        Cmd::MoveDown=>{
            let view = views.get_mut(vidx);
            let buffer = buffers.get(bidx);
            let len_lines = buffer.buf.len_lines();
            let line = buffer.buf.char_to_line(view.cursor);
            if line + 1 < len_lines{
                let line = line + 1;
                let start = buffer.buf.line_to_char(line);
                let len = buffer.buf.line(line).len_chars();
                let col = view.prefered_x.min(len.saturating_sub(1));
                view.cursor = start + col;
                View::scroll(view, buffers.get_mut(bidx));
            }
            Ok(())
        },
        Cmd::MoveRight=>{
            let buffer = buffers.get(bidx);
            let view = views.get_mut(vidx);
            let line = buffer.buf.char_to_line(view.cursor);
            let line_start = buffer.buf.line_to_char(line);
            let line_len = buffer.buf.line(line).len_chars();
            if view.cursor < line_start + line_len.saturating_sub(1){
                let col = view.cursor - line_start; 
                let col = col + 1;
                let col = col.min(buffer.buf.line(line).len_chars().saturating_sub(1));
                view.prefered_x = col;
                view.cursor = line_start + col;
            }
            Ok(())
        },
        Cmd::MoveLeft=>{
            let buffer = buffers.get(bidx);
            let view = views.get_mut(vidx);
            let line = buffer.buf.char_to_line(view.cursor);
            let line_start = buffer.buf.line_to_char(line);
            let col = view.cursor - line_start;
            if view.cursor > line_start {
                let col = col - 1;
                view.prefered_x = col;
                view.cursor = line_start + col;
            }
            Ok(())
        },
        Cmd::Split=>{
            let view = views.push(View::new(Some(SCRATCH), ViewKind::Text));
            let gidx = groups.push(Group::new(views, view, &[ViewKind::StatusBar, ViewKind::LineNumber]));
            let parent = get_parent(*focus)?;
            Nodes::new_leaf(nodes, gidx, parent, views, groups, focus);
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::Vsplit=>{
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::Hsplit=>{
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::ViewClose=>{
            let Node::Leaf {parent, generation, ..} = nodes.data.get_mut(focus.idx).unwrap()else{
                return Err(EditorErr::InvalidFocus);
            };
            *generation += 1;
            reflow(*parent, nodes, views, groups, focus);
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusDown=>{
            let p = get_parent(*focus)?;
            let Node::Branch {direction, parent, ..} = nodes.data.get(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
            match direction{
                Direction::Horizontal=>{
                    let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                    *f = (*f+1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
                Direction::Vertical=>{
                    if let Some(p) = parent{
                        let p = get_parent(*p)?;
                        let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                        *f = (*f+1)%children.len();
                        *focus = *children.get(*f).unwrap();
                    }
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusUp=>{
            let p = get_parent(*focus)?;
            let Node::Branch {direction, parent, ..} = nodes.data.get(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
            match direction{
                Direction::Horizontal=>{
                    let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                    *f = (*f+children.len()-1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
                Direction::Vertical=>{
                    if let Some(p) = parent{
                        let p = get_parent(*p)?;
                        let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                        *f = (*f+children.len()-1)%children.len();
                        *focus = *children.get(*f).unwrap();
                    }
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusRight=>{
            let p = get_parent(*focus)?;
            let Node::Branch {direction, parent, ..} = nodes.data.get(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
            match direction{
                Direction::Vertical=>{
                    let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                    *f = (*f+1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
                Direction::Horizontal=>{
                    if let Some(p) = parent{
                        let p = get_parent(*p)?;
                        let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                        *f = (*f+1)%children.len();
                        *focus = *children.get(*f).unwrap();
                    }
                }
            }
            enter_normal(cmd_line, mode);
            Ok(())
        }
        Cmd::FocusLeft=>{
            let p = get_parent(*focus)?;
            let Node::Branch {direction, parent, ..} = nodes.data.get(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
            match direction{
                Direction::Vertical=>{
                    let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                    *f = (*f+children.len()-1)%children.len();
                    *focus = *children.get(*f).unwrap();
                }
                Direction::Horizontal=>{
                    if let Some(p) = parent{
                        let p = get_parent(*p)?;
                        let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap() else {return Err(EditorErr::InvalidFocus);};
                        *f = (*f+children.len()-1)%children.len();
                        *focus = *children.get(*f).unwrap();
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
            let buffer = buffers.get_mut(bidx);
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
            let view = views.get_mut(vidx);
            let mut idx = {
                if let Some(idx) = buffer_idx{
                    idx
                }else{
                    if let Some(buf) = view.buf{
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
                    if view.buf == Some(idx){
                        view.buf = Some(SCRATCH);
                        cmd_line.input.clear();
                        view.off = 0;
                        view.cursor = 0;
                        view.prefered_x = 0;
                    }
                    buffers.remove(&mut idx);
                }
                view.dirty.extend(0..view.height as usize+1);
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
                let view = views.get_mut(vidx);
                view.buf = Some(idx);
                view.cursor = 0;
                view.off = 0;
                view.prefered_x = 0;
                view.dirty.extend(0..view.height as usize+1);
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
            let view = views.get_mut(vidx);
            view.off = 0;
            view.cursor = 0;
            view.prefered_x = 0;
            view.dirty.extend(0..view.height as usize+1);
            view.buf = Some(buffer);
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
        "focusdown"|"fd"=>Ok(Cmd::FocusDown),
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
    let root = {
        let new = Node::Branch { parent: None, children: vec![], direction: Direction::Vertical, focus: 0, generation: 0, pos_x: 0, pos_y: 0, width, height };
        nodes.push(new)
    };
    if let Node::Branch {height: h, width: w, ..} = nodes.data.get_mut(root.idx).unwrap(){
        *w = width;
        *h = height;
    }
    let mut focus = root;
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
        focus = Nodes::new_leaf(&mut nodes, gidx, ROOT, &mut views, &mut groups, &mut focus);
        reflow(root, &mut nodes, &mut views, &mut groups, &mut focus);
    }
    enable_raw_mode()?;
    execute!(stdout(), terminal::EnterAlternateScreen)?;

    //inital draw
    cmd_line.draw(mode)?;
    paint(root, &mode, &nodes, &mut views, &groups, &buffers)?;
    stdout().flush().unwrap();

    loop{
        if let Event::Key(event) = read()?{
            let cmd = key_to_cmd(event, &mode);
            match exec_cmd(&mut nodes, &mut focus, &mut cmd_line, &mut views, &mut buffers, &mut groups, cmd, &mut mode){
                Err(EditorErr::Msg(msg))=>cmd_line.draw_error(&mut mode, &msg)?,
                Err(EditorErr::Dirty(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is dirty",idx.idx))?,
                Err(EditorErr::InvalidBuffer)=>cmd_line.draw_error(&mut mode, "index is invalid")?,
                Err(EditorErr::ReadOnly(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is read only",idx.idx))?,
                Err(EditorErr::InvalidFocus)=>cmd_line.draw_error(&mut mode, "invalid focus")?,
                Err(EditorErr::Log(msg))=>log(&msg),
                Err(EditorErr::Io(_))=>break,
                Err(EditorErr::Quit)=>break,
                Ok(_) => {},
            }
            queue!(stdout(), cursor::Hide)?;
            reflow(root, &mut nodes, &mut views, &mut groups, &mut focus);
            paint(root, &mode, &nodes, &mut views, &groups, &buffers)?;
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
    execute!(stdout(), terminal::LeaveAlternateScreen).unwrap();
    Ok(())
}
