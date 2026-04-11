use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::path::Path;
use std::process::{exit};
use crossterm::event::PopKeyboardEnhancementFlags;
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
        cursor_x:usize,
        cursor_y:usize,
        text:String,
    },
    Delete{
        idx:usize,
        cursor_x:usize,
        cursor_y:usize,
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
        let cursor_char = view.cursor_char(self);
        self.buf.insert_char(cursor_char, c);
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

#[derive(Clone)]
struct View{
    buf: Option<BufferIdx>,
    dirty: Vec<usize>,
    y: usize,
    x: usize,
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
            x: 0,
            prefered_x: 0,
            y: 0,
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
        write!(out, "{}{}", termion::cursor::Goto(self.pos_x, self.pos_y+1), termion::clear::CurrentLine)?;
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
    fn draw_line_numbers(&self, views: &Views, parent_vidx: ViewIdx) -> io::Result<()> {
        let mut out = io::stdout().lock();

        if views.get(parent_vidx).dirty.is_empty(){
            return Ok(());
        }
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
            write!(out, "{}", termion::cursor::Goto(self.pos_x+1, self.pos_y + *row as u16 + 1))?;
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
        self.dirty.clear();
        let screen_y = self.pos_y + self.y.saturating_sub(self.off) as u16;
        let screen_x = self.pos_x + self.x as u16;
        write!(out, "{}", termion::cursor::Goto(screen_x+1, screen_y+1))?;
        out.flush()?;
        Ok(())
    }
    fn scroll(&mut self, buffer: &mut Buffer){
        if self.y < self.off{
            self.off = self.y;
            self.dirty.extend(0..self.height as usize + 1);
        } else if self.y > self.off + self.height as usize{
            self.off = self.y - self.height as usize;
            self.dirty.extend(0..self.height as usize + 1);
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
    generation: u64,
    parent: ViewIdx,
    children: Vec<ViewIdx>,
    width: u16,
    height: u16,
    pos_x: u16,
    pos_y: u16,
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
            height:0,
            width:0,
            pos_x:0,
            pos_y:0,
        }
    }
    fn sync(&self, views: &mut Views){
        let (y, off) = {
            let parent = &views.get(self.parent);
            (parent.y, parent.off)
        };
        for &child in &self.children{
            let child = views.get_mut(child);
            match child.kind{
                ViewKind::LineNumber=>{
                    child.y = y;
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
        self.height = height;
        self.width = width;
        self.pos_x = pos_x;
        self.pos_y = pos_y;
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
            let p = views.get_mut(self.parent);
            p.pos_x = pos_x;
            p.pos_y = pos_y;
            p.width = width;
            p.height = height;
            p.dirty.extend(p.y-p.off..p.height as usize);
        }
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
enum SplitDirection{
    Horizontal,
    Vertical,
}
#[derive(Clone)]
enum Node{
    Container{
        parent: NodeIdx,
        direction: SplitDirection,
        children: Vec<NodeIdx>,
        focus: usize,
        pos_x:  u16,
        pos_y:  u16,
        width:  u16,
        height: u16,
    },
    Leaf{
        parent: NodeIdx,
        gidx:GroupIdx
    }
}
fn draw(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views, buffers: &Buffers, groups: &mut Groups, mode: Mode)->Result<(), EditorErr>{
    match nodes.get(nidx).clone(){
        Node::Leaf{gidx, ..}=>{
            if gidx.generation == groups.get(gidx).generation{
                groups.get(gidx).sync(views);
                groups.get(gidx).draw_group(mode, views, buffers)?;
            }else{
                // Node::remove(nidx, parent, nodes);
                // Node::recalc(parent, nodes, views, groups);
            }
        },
        Node::Container{children, focus: f, ..}=>{
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
    fn recalc(nidx: NodeIdx, nodes: &Nodes, views: &mut Views, groups: &mut Groups){
        if let Node::Container { direction, pos_x, pos_y, width, height, children, ..} = nodes.get(nidx){
            let mut remainder = {
                match direction {
                    SplitDirection::Vertical=>(*height-1)/children.len()as u16%*height,
                    SplitDirection::Horizontal=>*width/children.len()as u16%*width,
                }
            };
            let height = {
                match direction{
                    SplitDirection::Horizontal=>(*height as usize-1)/children.len(),
                    SplitDirection::Vertical=>*height as usize,
                }
            };
            let width = {
                match direction {
                    SplitDirection::Vertical=>*width as usize/children.len(),
                    SplitDirection::Horizontal=>*width as usize,
                }
            };
            let mut pos_x = *pos_x;
            let mut pos_y = *pos_y;
            for c in children{
                let c = nodes.get(*c);
                match c{
                    Node::Leaf{gidx, ..}=>{
                        match direction{
                            SplitDirection::Horizontal=>{
                                if remainder > 0{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16, pos_x, pos_y, views);
                                    remainder -= 1;
                                    pos_y += height as u16+1;
                                }else{
                                    groups.get_mut(*gidx).resize(height as u16, width as u16, pos_x, pos_y, views);
                                    pos_y += height as u16;
                                }
                            },
                            SplitDirection::Vertical=>{
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
        let new = nodes.push(Node::Leaf{gidx, parent:container});
        if let Node::Container {children, ..} = nodes.get_mut(container){
            children.push(new);
        }
        Node::recalc(container, nodes, views, groups);
        new
    }
    fn leaf_to_container(nidx: NodeIdx, nodes: &mut Nodes, groups: &mut Groups, direction: SplitDirection){
        if let Node::Leaf {gidx, parent, ..} = nodes.get(nidx){
            let Group {width, height, pos_x, pos_y, ..} = groups.get(*gidx);
            *nodes.get_mut(nidx) = Node::Container{parent: *parent, direction, children: vec![], focus:0, pos_x: *pos_x, pos_y:*pos_y, width:*width, height:*height};
        }
    }
    fn remove(remove: NodeIdx, parent: NodeIdx, nodes: &mut Nodes){
        if let Node::Container {children, ..} = nodes.get_mut(parent){
            children.retain(|x| *x != remove);
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
    // Split,
    // Vsplit,
    // Hsplit,
    // FocusUp,
    // FocusDown,
    // FocusRight,
    // FocusLeft,
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
                        Key::Char('u') => Cmd::Undo,
                        Key::Char('U') => Cmd::Redo,
                        Key::Char('h') | Key::Left => Cmd::MoveLeft,
                        Key::Char('j') | Key::Down => Cmd::MoveDown,
                        Key::Char('k') | Key::Up => Cmd::MoveUp,
                        Key::Char('l') | Key::Right => Cmd::MoveRight,
                        // Key::Ctrl('h') | Key::CtrlLeft => Cmd::FocusLeft,
                        // Key::Ctrl('j') | Key::CtrlDown=> Cmd::FocusDown,
                        // Key::Ctrl('k') | Key::CtrlUp => Cmd::FocusUp,
                        // Key::Ctrl('l') | Key::CtrlRight => Cmd::FocusRight,
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

fn exec_cmd(nodes: &mut Nodes, focus: &mut NodeIdx, cmd_line: &mut CmdLine, views: &mut Views, buffers: &mut Buffers, groups: &mut Groups, cmd: Cmd, mode: &mut Mode)->Result<(), EditorErr>{
    fn enter_normal(cmd_line: &mut CmdLine, mode: &mut Mode){
        cmd_line.cursor = 0;
        *mode = Mode::Normal;
    }
    // fn get_parent_idx(nodes: &Nodes, nidx: &NodeIdx)->NodeIdx{
    //     match nodes.get(*nidx){
    //         Node::Leaf { parent, ..}=>{
    //             *parent
    //         },
    //         Node::Container {parent, ..}=>{
    //             *parent
    //         },
    //     }
    // }
    // fn focus_next(nodes: &mut Nodes, focus: &mut NodeIdx){
    //     let parent = get_parent_idx(nodes, focus);
    //     if let Node::Container {children, focus: f, .. } = nodes.get_mut(parent){
    //         *f = (*f+1)%children.len();
    //         *focus = *children.get(*f).unwrap();
    //     }
    // }
    // fn focus_prev(nodes: &mut Nodes, focus: &mut NodeIdx){
    //     let parent = get_parent_idx(nodes, focus);
    //     if let Node::Container {children, focus:f, ..} = nodes.get_mut(parent){
    //         *f = ((*f+children.len())-1)%children.len();
    //         *focus = *children.get(*f).unwrap();
    //     }
    // }
    // fn focus_next_parent(nodes: &mut Nodes, focus: &mut NodeIdx){
    //     let parent = get_parent_idx(nodes, focus);
    //     let parent = get_parent_idx(nodes, &parent);
    //     if let Node::Container {children, focus: f, ..} = nodes.get_mut(parent){
    //         *f = (*f+1)%children.len();
    //         *focus = *children.get(*f).unwrap();
    //     }
    // }
    // fn focus_prev_parent(nodes: &mut Nodes, focus: &mut NodeIdx){
    //     let parent = get_parent_idx(nodes, focus);
    //     let parent = get_parent_idx(nodes, &parent);
    //     if let Node::Container {children, focus: f, ..} = nodes.get_mut(parent){
    //         *f = ((*f+children.len())-1)%children.len();
    //         *focus = *children.get(*f).unwrap();
    //     }
    // }
    if let Node::Container{children, focus: f, ..}= nodes.get(*focus){
        *focus = children[*f];
    };
    let group = {
        match nodes.get_mut(*focus) {
            Node::Leaf{gidx, ..}=>gidx,
            _ => return Ok(())
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
            buffer.redo.clear();
            let idx = curr_view.cursor_char(buffer);
            let cursor_x = curr_view.x;
            let cursor_y = curr_view.y;
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
                            buffer.undo.push(Edit::Insert { idx, cursor_x, cursor_y, text: c.into() });
                        }
                    }
                    Edit::Delete {..}=>{
                        buffer.undo.push(Edit::Insert { idx, cursor_x, cursor_y, text: c.into() });
                    }
                }
            }else{
                buffer.undo.push(Edit::Insert { idx, cursor_x, cursor_y, text: c.into()});
            }
            buffer.insert(views.get(view), c);
            let view = views.get_mut(view);
            view.x += 1;
            view.prefered_x = view.x;
            view.dirty.push(view.y-view.off);
            Ok(())
        },
        Cmd::NewLine=>{
            buffer.redo.clear();
            let idx = curr_view.cursor_char(buffer);
            buffer.insert(&mut curr_view, '\n');
            buffer.undo.push(Edit::Insert { idx, cursor_x: curr_view.x, cursor_y: curr_view.y, text: "\n".to_string()});
            curr_view.dirty.extend(curr_view.y-curr_view.off..curr_view.height as usize);
            curr_view.y += 1;
            curr_view.x = 0;
            View::scroll(&mut curr_view, buffer);
            Ok(())
        },
        Cmd::Backspace=>{
            buffer.redo.clear();
            let idx = curr_view.cursor_char(buffer);
            if idx != 0{
                let del = buffer.buf.slice(idx-1..idx).to_string();
                if let Some(edit) = buffer.undo.last_mut(){
                    match edit{
                        Edit::Insert {..}=>{
                            buffer.undo.push(Edit::Delete { idx:idx-1, cursor_x: curr_view.x, cursor_y: curr_view.y, text: del });
                        },
                        Edit::Delete { idx: xidx, text, .. }=>{
                            if *xidx == idx{
                                *xidx -= 1;
                                text.insert_str(0, &del);
                            }else{
                                buffer.undo.push(Edit::Delete { idx: idx - 1, cursor_x: curr_view.x, cursor_y: curr_view.y, text: del});
                            }
                        }
                    }
                }else{
                    buffer.undo.push(Edit::Delete { idx: idx-1, cursor_x: curr_view.x, cursor_y: curr_view.y, text: del});
                }
                buffer.buf.remove(idx - 1..idx);
                if curr_view.x > 0{
                    curr_view.x -= 1;
                    curr_view.prefered_x = curr_view.x;
                    curr_view.dirty.push(curr_view.y-curr_view.off);
                }else{
                    curr_view.y = curr_view.y.saturating_sub(1);
                    if let Some(line) = buffer.buf.get_line(curr_view.y-curr_view.off){
                        curr_view.x = line.len_chars();
                    }
                    curr_view.dirty.extend(curr_view.y..curr_view.off+curr_view.height as usize);
                }
                View::scroll(&mut curr_view, buffer);
            }
            Ok(())
        },
        Cmd::Undo=>{
            if let Some(edit) = buffer.undo.pop(){
                match edit{
                    Edit::Insert { idx, cursor_x, cursor_y, text, }=>{
                        buffer.redo.push(Edit::Delete { idx, cursor_y, cursor_x, text: text.clone() });
                        buffer.buf.remove(idx..idx + text.chars().count());
                        curr_view.y = cursor_y;
                        curr_view.x = cursor_x;
                        curr_view.prefered_x = curr_view.x;
                    },
                    Edit::Delete { idx, cursor_x, cursor_y, text, }=>{
                        buffer.redo.push(Edit::Insert {idx, cursor_x, cursor_y, text: text.clone()});
                        buffer.buf.insert(idx, &text);
                        curr_view.y = cursor_y;
                        curr_view.x = cursor_x;
                        curr_view.prefered_x = curr_view.x;
                    },
                }
                curr_view.dirty.extend(curr_view.y-curr_view.off..curr_view.height as usize);
                return Ok(())
            }
            Err(EditorErr::Msg("undo stack is empty".to_string()))
        },
        Cmd::Redo => {
            if let Some(edit) = buffer.redo.pop() {
                match edit {
                    Edit::Insert { idx, cursor_x, cursor_y, text } => {
                        buffer.buf.remove(idx..idx + text.chars().count());
                        curr_view.x = curr_view.x.saturating_sub(text.chars().count());
                        curr_view.y = cursor_y;
                        curr_view.prefered_x = curr_view.x;
                        buffer.undo.push(Edit::Delete{ idx, cursor_x, cursor_y, text });
                    }
                    Edit::Delete { idx, cursor_x, cursor_y, text } => {
                        buffer.buf.insert(idx, &text);
                        curr_view.y = cursor_y;
                        curr_view.x = cursor_x;
                        curr_view.prefered_x = curr_view.x;
                        buffer.undo.push(Edit::Insert{ idx, cursor_x, cursor_y, text });
                    }
                }
                curr_view.dirty.extend(curr_view.y-curr_view.off..curr_view.height as usize);
                View::scroll(curr_view, buffer); // Make sure the view scrolls correctly
                return Ok(());
            }
            Err(EditorErr::Msg("redo stack is empty".to_string()))
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
        // Cmd::Split=>{
        //     let view = views.push(View::new(Some(SCRATCH), ViewKind::Text));
        //     let gidx = groups.push(Group::new(views, view, &[ViewKind::StatusBar, ViewKind::LineNumber]));
        //     let parent = get_parent_idx(nodes, focus);
        //     Node::add_leaf(parent, nodes, views, groups, gidx);
        //     write!(stdout().lock(), "{}", termion::clear::All)?;
        //     focus_next(nodes, focus);
        //     cmd_line.cursor = 0;
        //     *mode = Mode::Normal;
        //     Ok(())
        // }
        // Cmd::Vsplit=>{
        //     let group = group.clone();
        //     Node::leaf_to_container(*focus, nodes, groups, SplitDirection::Vertical);
        //     Node::add_leaf(*focus, nodes, views, groups, group);
        //     let parent = views.push(View::new(Some(SCRATCH), ViewKind::Text));
        //     let gidx = groups.push(Group::new(views, parent, &[ViewKind::StatusBar, ViewKind::LineNumber]));
        //     *focus = Node::add_leaf(*focus, nodes, views, groups, gidx);
        //     focus_next(nodes, focus);
        //     cmd_line.cursor = 0;
        //     *mode = Mode::Normal;
        //     Ok(())
        // }
        // Cmd::Hsplit=>{
        //     let group = group.clone();
        //     Node::leaf_to_container(*focus, nodes, groups, SplitDirection::Horizontal);
        //     Node::add_leaf(*focus, nodes, views, groups, group);
        //     let parent = views.push(View::new(Some(SCRATCH), ViewKind::Text));
        //     let gidx = groups.push(Group::new(views, parent, &[ViewKind::StatusBar, ViewKind::LineNumber]));
        //     *focus = Node::add_leaf(*focus, nodes, views, groups, gidx);
        //     focus_next(nodes, focus);
        //     cmd_line.cursor = 0;
        //     *mode = Mode::Normal;
        //     Ok(())
        // }
        // Cmd::FocusDown=>{
        //     let parent = get_parent_idx(nodes, focus);
        //     if let Node::Container { direction, ..} = nodes.get(parent){
        //         match direction{
        //             SplitDirection::Vertical=>focus_next_parent(nodes, focus),
        //             SplitDirection::Horizontal=>focus_next(nodes, focus),
        //         }
        //     }
        //     enter_normal(cmd_line, mode);
        //     Ok(())
        // }
        // Cmd::FocusUp=>{
        //     let parent = get_parent_idx(nodes, focus);
        //     if let Node::Container {direction, ..} = nodes.get(parent){
        //         match direction{
        //             SplitDirection::Vertical=>focus_prev_parent(nodes, focus),
        //             SplitDirection::Horizontal=>focus_prev(nodes, focus),
        //         }
        //     }
        //     enter_normal(cmd_line, mode);
        //     Ok(())
        // }
        // Cmd::FocusRight=>{
        //     let parent = get_parent_idx(nodes, focus);
        //     if let Node::Container {direction, focus: f, children, ..} = nodes.get(parent){
        //         if *f == children.len(){
        //             focus_next_parent(nodes, focus);
        //         }else{
        //             match direction{
        //                 SplitDirection::Horizontal=>focus_next_parent(nodes, focus),
        //                 SplitDirection::Vertical=>focus_next(nodes, focus),
        //             }
        //         }
        //     }
        //     enter_normal(cmd_line, mode);
        //     Ok(())
        // }
        // Cmd::FocusLeft=>{
        //     let parent = get_parent_idx(nodes, focus);
        //     if let Node::Container {direction, focus: f, ..} = nodes.get(parent){
        //         if *f == 0{
        //             focus_prev_parent(nodes, focus);
        //         }else{
        //             match direction {
        //                 SplitDirection::Horizontal=>focus_prev_parent(nodes, focus),
        //                 SplitDirection::Vertical=>focus_prev(nodes, focus),
        //             }
        //         }
        //     }
        //     enter_normal(cmd_line, mode);
        //     Ok(())
        // }
        Cmd::CmdInsert(c) => {
            if c != '\n' {
                cmd_line.insert(c);
                cmd_line.draw(*mode)?;
            }else{
                let input = cmd_line.input.clone();
                match parse_cmd(input){
                    Ok(parsed_cmd) => exec_cmd(nodes, focus, cmd_line, views, buffers, groups, parsed_cmd, mode)?,
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
                        curr_view.x = 0;
                        curr_view.y = 0;
                        curr_view.prefered_x = 0;
                    }
                    buffers.remove(&mut idx);
                }
                curr_view.dirty.extend(0..curr_view.height as usize +1);
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
                    .filter(|(i, b)| !b.undo.is_empty() && *i != SCRATCH.idx)
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
                curr_view.dirty.extend(0..curr_view.height as usize +1);
                cmd_line.input.clear();
                cmd_line.cursor = 0;
                *mode = Mode::Normal;
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
            curr_view.x = 0;
            curr_view.y = 0;
            curr_view.prefered_x = 0;
            curr_view.dirty.extend(0..curr_view.height as usize +1);
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
        // "split"  | "s"  =>Ok(Cmd::Split),
        // "splitv" | "sv" =>Ok(Cmd::Vsplit),
        // "splith" | "sh" =>Ok(Cmd::Hsplit),
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
    //     "right"=> Ok(Cmd::FocusRight),
    //     "left" => Ok(Cmd::FocusLeft),
    //     "down" => Ok(Cmd::FocusDown),
    //     "up"   => Ok(Cmd::FocusUp),
        _ => Err(EditorErr::Msg(format!("unknown command: {}",cmd))),
    }
}

const SCRATCH: BufferIdx = BufferIdx{idx: 0, generation: 0};
fn main()->io::Result<()>{
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut groups = Groups::new();
    let mut nodes = Nodes{data:vec![], free:vec![]};
    let (width, height) = termion::terminal_size().unwrap();
    let mut cmd_line = CmdLine::new(height);
    let height = height -2;
    let mut mode = Mode::Normal;
    let root = nodes.push(Node::Container{parent: NodeIdx{idx:0}, direction: SplitDirection::Vertical, width, height, pos_x:0, pos_y:0, focus:0, children:vec![]});
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
    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::clear::All)?;

    //inital draw
    cmd_line.draw(mode)?;
    draw(root, &mut nodes, &mut views, &buffers, &mut groups, mode).unwrap();
    let mut focus = NodeIdx{idx:1};

    for key in input.keys(){
        let cmd = key_to_cmd(key?, &mode);
        match exec_cmd(&mut nodes, &mut focus, &mut cmd_line, &mut views, &mut buffers, &mut groups, cmd, &mut mode){
            Err(EditorErr::Msg(msg))=>cmd_line.draw_error(&mut mode, &msg)?,
            Err(EditorErr::Dirty(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is dirty",idx.idx))?,
            Err(EditorErr::InvalidBuffer)=>cmd_line.draw_error(&mut mode, "index is invalid")?,
            Err(EditorErr::ReadOnly(idx))=>cmd_line.draw_error(&mut mode, &format!("buffer:{} is read only",idx.idx))?,
            Err(EditorErr::Io(_))=>exit(1),
            Ok(_) => {},
        }
        draw(root, &mut nodes, &mut views, &buffers, &mut groups, mode).unwrap();
        match mode{
            Mode::Command =>{
                cmd_line.draw(mode)?
            } 
            _ => {},
        }
    }
    Ok(())
}
