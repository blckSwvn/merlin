use core::panic;
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
use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use crossterm::execute;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal;
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

struct Logger{
    file: &'static str,
}
impl Logger{
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferIdx{
    idx:usize,
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
            let idx = BufferIdx{idx:self.data.len()};
            self.data.push(buf);
            idx
        }else{
            let idx = self.free.pop_front().unwrap();
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
                return Some(idx)
            }
        }
        None
    }
    fn remove(&mut self, idx: &mut BufferIdx){
        self.get_mut(*idx).generation += 1;
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

#[derive(Clone)]
struct Rect{
    x:u16,
    y:u16,
    height:u16,
    width:u16,
}
#[derive(Clone)]
enum Decoration{
    LineNumber(Rect),
    StatusBar(Rect),
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
    last_off: usize,
    last_cursor: usize,
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
            last_off: 0,
            last_cursor: 0,
            file: path.map(PathBuf::from),
            redo: Vec::new(),
            undo: Vec::new(),
        })
    }
    fn insert(&mut self, off: usize, cursor: usize, c: char){
        self.last_cursor = cursor;
        self.last_off = off;
        self.buf.insert_char(cursor, c);
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
    error: bool,
}
impl CmdLine{
    fn new(height: u16)->Self{
        Self{
            input: String::new(),
            pos_y: height,
            cursor: 0,
            error: false,
        }
    }
    fn insert(&mut self, c: char){
        if self.error{
            self.cursor = 0;
            self.input.clear();
            self.error = false;
        }
        let byte_idx = self.cursor;
        self.input.insert(byte_idx, c);
        self.cursor += c.len_utf8();
    }
    fn backspace(&mut self){
        if self.error{
            self.cursor = 0;
            self.input.clear();
            self.error = false;
        }
        if self.cursor > 0 {
            let char_len = self.input[..self.cursor].chars().rev().next().unwrap().len_utf8();
            self.cursor -= char_len as usize;
            self.input.remove(self.cursor);
        }
    }
    fn error(&mut self, s: &str){
        self.error = true;
        self.input.clear();
        self.input = s.to_string();
    }
    fn draw(&self, focus: &Focus, screen: &mut ScreenBuffer){
        let s = {
            if self.error{
                format!("{}",self.input)
            }else{
                if let Focus::CmdLine = focus{
                    format!(":{}",self.input)
                }else{
                    if self.input.is_empty(){
                        "".to_string()
                    }else{
                        format!(":{}",self.input)
                    }
                }
            }
        };
        screen.set_string_xy(0, self.pos_y, &s);
    }
}

struct View{
    buf: BufferIdx,
    cursor: usize,
    prefered_x: usize,
    off: usize,
    rect: Rect,
    recalc: fn(&mut View, &mut Rect),
    draw: fn(&View, buffer: &Buffers, &mut ScreenBuffer),
    mode: Mode,
    deco: Vec<Decoration>,
}

impl View{
    fn recalc(&mut self, rect: &mut Rect){
        deco_recalc(&mut self.deco, rect);
        view_recalc(self, rect);
    fn deco_recalc(deco: &mut Vec<Decoration>, rect: &mut Rect){
        for d in deco{
            match d{
                Decoration::LineNumber(r)=>{
                    r.x = rect.x;
                    r.y = rect.y;
                    r.height = rect.height-1;
                    r.width = 5;
                    rect.width -= 5;
                    rect.x += 5;
                }
                Decoration::StatusBar(r)=>{
                    r.x = rect.x;
                    r.y = rect.y + rect.height-1;
                    r.width = rect.width;
                    rect.height -= 1;
                }
            }
        }
    }
    fn view_recalc(view: &mut View, rect: &mut Rect){
        view.rect.height = rect.height-1;
        view.rect.width = rect.width;
        view.rect.x = rect.x;
        view.rect.y = rect.y;
    }
    }
    fn draw(&self, buffers: &Buffers, screen: &mut ScreenBuffer){
        deco_draw(&self, buffers, screen);
        text_draw(&self, buffers, screen);
        fn text_draw(view: &View, buffers: &Buffers, screen: &mut ScreenBuffer){
            for row in 0..view.rect.height+1{
                let line_idx = view.off + row as usize;
                if let Some(line) = buffers.get(view.buf).buf.get_line(line_idx){
                    let end = usize::min(view.rect.width as usize, line.len_chars());
                    let s = line.slice(..end.saturating_sub(1));
                    screen.set_string_xy(view.rect.x, view.rect.y + row, &s.to_string());
                }
            }
        }
        fn deco_draw(view: &View, buffers: &Buffers, screen: &mut ScreenBuffer){
            for d in &view.deco{
                match d{
                    Decoration::LineNumber(r)=>{
                        for row in 0..r.height+1{
                            let screen_y = r.y + row as u16;
                            let line_num = view.off+row as usize;
                            let s = format!("{:>width$} ", line_num,
                                width = r.width as usize -1);
                            screen.set_string_xy(r.x, screen_y, &s);
                        }
                    }
                    Decoration::StatusBar(r)=>{
                        let mut path = "SCRATCH";
                        let buffer = buffers.get(view.buf);
                        if !buffer.check_flag(Buffer::SCRATCH){
                            if let Some(p) = &buffer.file{
                                path = p.to_str().unwrap_or("NEW_FILE");
                            }else{
                                path = "NEW_FILE";
                            }
                        }
                        let mode_str = match view.mode{
                            Mode::Normal=>"NOR",
                            Mode::Insert=>"INS",
                        };
                        let s = format!("{mode_str} {} {path}", view.buf.idx);
                        let s = format!("{:width$}", s, width = r.width as usize);
                        screen.set_string_xy(r.x, r.y, &s);
                    }
                }
            }
        }
    }
    fn new(buf: BufferIdx, deco: &[Decoration])->Self{
        Self{
            buf,
            cursor: 0,
            prefered_x: 0,
            off: 0,
            rect: Rect { x:0, y:0, height:0, width:0},
            draw: View::draw,
            // draw: View::draw,
            recalc: View::recalc,
            mode: Mode::Normal,
            deco: deco.to_vec(),
        }
    }
    fn scroll(&mut self, buffer: &mut Buffer){
        let line = buffer.buf.char_to_line(self.cursor);
        let line_start = buffer.buf.line_to_char(line);
        if line < self.off{
            self.off = line;
        } else if line > self.off + self.rect.height as usize{
            self.off = line - self.rect.height as usize;
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

enum Node{
    Leaf{
        parent: NodeIdx,
        vidx: ViewIdx,
    },
    Branch{
        parent: Option<NodeIdx>,
        children: Vec<NodeIdx>,
        direction: Direction,
        focus: usize,
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
}

enum Focus{
    Node(NodeIdx),
    CmdLine,
}

struct Nodes{
    data:Vec<Node>,
    root:Vec<NodeIdx>,
    free:Vec<usize>,
}

impl Nodes{
    fn new_leaf(&mut self, parent: NodeIdx, vidx: ViewIdx, views: &mut Views)->NodeIdx{
        let nidx = self.push(Node::Leaf { parent, vidx});
        let Node::Branch{children, ..} = self.data.get_mut(parent.idx).unwrap()else {panic!()};
        children.push(nidx);
        recalc(parent, self, views);
        nidx
    }
fn new_root(&mut self, pos_x: u16, pos_y: u16, height: u16, width: u16, direction: Direction)->NodeIdx{
        let root = self.push(Node::Branch{
            parent: None,
            children: vec![],
            direction,
            focus: 0, pos_x, pos_y, width, height
        });
        self.root.push(root);
        root
    }
    fn new_branch(&mut self, parent: NodeIdx, vidx: ViewIdx, views: &mut Views, direction: Direction){
        let new = self.push(Node::Branch {
            parent: Some(parent),
            children: vec![],
            direction,
            focus: 0, pos_x: 0, pos_y: 0, width: 0, height: 0
        });
        let nidx = self.push(Node::Leaf { parent:new, vidx});
        let Node::Branch{children, ..} = self.data.get_mut(new.idx).unwrap()else {panic!()};
        children.push(nidx);
        let Node::Branch {children, ..} = self.data.get_mut(parent.idx).unwrap()else{panic!()};
        children.push(new);
        recalc(parent, self, views);
        recalc(new, self, views);
    }
    fn push(&mut self, node: Node)->NodeIdx{
        if self.free.is_empty(){
            let idx = self.data.len();
            self.data.push(node);
            NodeIdx { idx }
        }else{
            let idx = self.free.pop().unwrap();
            self.data[idx] = node;
            NodeIdx { idx }
        }
    }
    fn remove(&mut self, nidx: NodeIdx){
        self.free.push(nidx.idx);
    }
}

struct ScreenBuffer{
    cells: Vec<char>,
    width: u16,
    height: u16,
    cursor_y: u16,
    cursor_x: u16,
}
impl ScreenBuffer{
    fn set_cell_xy(&mut self, x: u16, y: u16, cell: char){
        let idx = y * self.width + x;
        self.cells[idx as usize] = cell;
    }
    fn set_cell(&mut self, idx: usize, cell: char){
        self.cells[idx as usize] = cell;
    }
    fn get_cell_xy(&mut self, x: u16, y: u16)->char{
        let idx = y * self.width + x;
        self.cells[idx as usize]
    }
    fn get_cell(&mut self, idx: usize)->char{
        self.cells[idx as usize]
    }
    fn set_string_xy(&mut self, x: u16, y: u16, s: &str){
        for (i, cell) in s.chars().enumerate(){
            let xx = x + i as u16;
            if xx >= self.width || y >= self.height {
                break;
            }
            self.set_cell_xy(xx, y, cell);
        }
    }
    fn clear_buffer(&mut self){
        self.cells.fill(' ');
    }
    fn print(&mut self, prev: &mut ScreenBuffer)->io::Result<()>{
        let mut out = stdout().lock();
        for y in 0..self.height{
            let mut x = 0;
            while x < self.width{
                let idx = y * self.width + x;
                let old = prev.cells[idx as usize];
                let new = self.cells[idx as usize];
                if new != old{
                    let start_x = x;
                    let mut line = String::with_capacity(self.width as usize);
                    while x < self.width{
                        let idx = y * self.width + x;
                        let old = prev.cells[idx as usize];
                        let new = self.cells[idx as usize];
                        if new == old{
                            break;
                        }
                        line.push(new);
                        x += 1;
                    }
                    queue!(out, MoveTo(start_x, y), Print(line))?;
                }else{
                    x += 1;
                }
            }
        }
        queue!(out, MoveTo(self.cursor_x, self.cursor_y))?;
        std::mem::swap(self, prev);
        self.clear_buffer();
        Ok(())
    }
}
fn paint(mode: &Mode, focus: &mut Focus, cmd_line: &mut CmdLine, nodes: &Nodes, views: &Views, buffers: &Buffers, old: &mut ScreenBuffer, new: &mut ScreenBuffer)->io::Result<()>{
    for r in &nodes.root{
        draw(*r, mode, nodes, views, buffers, new)?;
    }
    cmd_line.draw(focus, new);
    new.print(old)?;
    match focus{
        Focus::Node(_)=>{
            let mut n = nodes.root.get(0).unwrap();
            while let Node::Branch {children, focus: f, ..} = nodes.data.get(n.idx).unwrap(){
                n = children.get(*f).unwrap();
            }
            *focus = Focus::Node(*n);
        }
        Focus::CmdLine=>{
                queue!(stdout(), MoveTo(cmd_line.cursor as u16 +1, cmd_line.pos_y))?;
            }
        }
    return Ok(());
    fn draw(nidx: NodeIdx, mode: &Mode, nodes: &Nodes, views: &Views, buffers: &Buffers, screen: &mut ScreenBuffer)->io::Result<()>{
        match nodes.data.get(nidx.idx).unwrap(){
            Node::Leaf {vidx, ..}=>{
                let view = views.get(*vidx);
                let buffer = buffers.get(view.buf);
                (view.draw)(view, buffers, screen);
                let line = buffer.buf.char_to_line(view.cursor);
                let screen_y = line.saturating_sub(view.off) + view.rect.y as usize;
                let line_start = buffer.buf.line_to_char(line);
                let col = view.cursor - line_start;
                screen.cursor_x = col as u16 + view.rect.x;
                screen.cursor_y = screen_y as u16;
            }
            Node::Branch {children, focus, ..}=>{
                for (i, c) in children.iter().enumerate(){
                    if i != *focus as usize{
                        draw(*c, mode, nodes, views, buffers, screen)?;
                    }
                }
                if !children.is_empty(){
                    let f = children.get(*focus).unwrap();
                    draw(*f, mode, nodes, views, buffers, screen)?;
                }
            }
        }
        Ok(())
    }
}

fn recalc(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views){
    let curr = nidx;
    let Node::Branch {children, direction, height, width, pos_x, pos_y, ..} = nodes.data.get(curr.idx).unwrap()else{
        panic!()
    };
    let (width, height, mut remainder) = {
        match direction{
            Direction::Vertical=> (width/children.len()as u16, *height, width%children.len() as u16),
            Direction::Horizontal=> (*width, *height/children.len()as u16, height%children.len() as u16),
        }
    };
    let mut pos_x = pos_x.clone();
    let mut pos_y = pos_y.clone();
    let direction = direction.clone();
    let children = children.clone();
    for c in children.iter(){
        let (width, height) = {
            match direction{
                Direction::Vertical=>{
                    if remainder > 0 {
                        (width+1, height)
                    }else{
                        (width, height)
                    }
                }
                Direction::Horizontal=>{
                    if remainder > 0{
                        (width, height+1)
                    }else{
                        (width, height)
                    }
                }
            }
        };
        let n = nodes.data.get_mut(c.idx).unwrap();
        match n{
            Node::Branch {width: w, height: h, pos_x: x, pos_y: y, ..}=>{
                *w = width;
                *h = height;
                *x = pos_x;
                *y = pos_y;
                recalc(*c, nodes, views);
            }
            Node::Leaf {vidx, ..}=>{
                let mut rect = Rect{x: pos_x, y: pos_y, width, height};
                let view = views.get_mut(*vidx);
                (view.recalc)(view, &mut rect);
            }
        }
        match direction{
            Direction::Vertical=>pos_x += width,
            Direction::Horizontal=>pos_y += height,
        }
        remainder = remainder.saturating_sub(1);
    }
}

fn reflow(nidx: NodeIdx, nodes: &mut Nodes, views: &mut Views, focus: &mut NodeIdx){
    let mut remove_nodes: Vec<(NodeIdx, usize, NodeIdx)> = vec![];
    let mut curr = nidx;
    while let Node::Branch {parent, children, ..} = nodes.data.get(curr.idx).unwrap(){
        for (c, n) in children.iter().enumerate(){
            if let Node::Branch {children, ..} = nodes.data.get(n.idx).unwrap(){
                if children.is_empty(){
                    remove_nodes.push((curr, c, *n));
                }
            }
        }
        if children.is_empty(){
            if let Some(p) = parent{
                curr = *p;
            }
        }else{
            break;
        }
    }
    for (p, c, n) in remove_nodes.iter(){
        let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap()else {return;};
        children.remove(*c);
        *f = (*f+children.len()-1)%children.len();
        *focus = *children.get(*f).unwrap();
        recalc(*p, nodes, views);
        nodes.remove(*n);
    }
}

#[derive(Clone, Copy)]
enum Mode{
    Normal,
    Insert,
}

fn key_to_exec(key: KeyEvent, nodes: &mut Nodes, focus: &mut Focus, cmd_line: &mut CmdLine, views: &mut Views, buffers: &mut Buffers)->Result<(), EditorErr>{
    match focus{
        Focus::CmdLine=>{
            let cmd = match key.code{
                KeyCode::Char(c)=>Cmd::Insert(c),
                KeyCode::Esc=>Cmd::EnterNormal,
                KeyCode::Backspace=>Cmd::BackSpace,
                KeyCode::Left=>Cmd::MoveLeft,
                KeyCode::Right=>Cmd::MoveRight,
                KeyCode::Enter=>Cmd::Exec,
                _ => Cmd::Noop,
            };
            exec_cmd(cmd, cmd_line, nodes, focus, views, buffers)?;
            enum Cmd{
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
            fn parse_cmd(s: String)->Result<Cmd, String>{
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
                let cmd = parts.next().ok_or(format!("unknown command: {}",s))?;
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
                                Ok(Cmd::SwitchBuffer(BufferIdx {idx}))
                            }else{
                                Ok(Cmd::Open(Some(arg.clone())))
                            }
                        }else{
                            Ok(Cmd::Open(None))
                        }
                    }
                    "split"  | "s"  =>Ok(Cmd::Split),
                    "splitv" | "sv" =>Ok(Cmd::SplitV),
                    "splith" | "sh" =>Ok(Cmd::SplitH),
                    "close" | "c" => {
                        let mut args = Vec::new();
                        args.push(rest);
                        if let Some(arg) = args.get(0){
                            if let Ok(idx) = arg.parse::<usize>(){
                                Ok(Cmd::Close(Some(BufferIdx {idx}), false))
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
                                Ok(Cmd::Close(Some(BufferIdx {idx}), true))
                            }else{
                                Ok(Cmd::Close(None, true))
                            }
                        }else{
                            Ok(Cmd::Close(None, true))
                        }
                    }
                    "viewclose" | "vc"=> Ok(Cmd::ViewClose),
                    _ => Err(format!("unknown command: {}",cmd)),
                }
            }
            fn exec_cmd(
                cmd: Cmd,
                cmd_line: &mut CmdLine,
                nodes: &mut Nodes,
                focus: &mut Focus,
                views: &mut Views,
                buffers: &mut Buffers
            )->Result<(), EditorErr>{
                let (bidx, vidx, mut nidx) = {
                    let mut nidx = &NodeIdx { idx: 0};
                    while let Node::Branch {children, focus, ..} = nodes.data.get(nidx.idx).unwrap(){
                        nidx = children.get(*focus).unwrap();
                    }
                    let Node::Leaf{vidx, ..} = nodes.data.get(nidx.idx).unwrap()else {panic!()};
                    let view = views.get(*vidx);
                    (view.buf, *vidx, *nidx)
                };
                fn enter_normal(focus: &mut Focus, nidx: NodeIdx, cmd_line: &mut CmdLine){
                    queue!(stdout(), cursor::SetCursorStyle::SteadyBlock).unwrap();
                    cmd_line.cursor = 0;
                    *focus = Focus::Node(nidx);
                }
                fn get_parent(nodes: &Nodes, nidx: NodeIdx)->Result<NodeIdx, EditorErr>{
                    let Node::Leaf {parent, ..} = nodes.data.get(nidx.idx).unwrap() else {return Err(EditorErr::InvalidFocus)};
                    Ok(*parent)
                }
                match cmd{
                    Cmd::Exec=>{
                        match parse_cmd(cmd_line.input.clone()){
                            Ok(cmd)=>{
                                exec_cmd(cmd, cmd_line, nodes, focus, views, buffers)?;
                            }
                            Err(s)=>{
                                *focus = Focus::Node(nidx);
                                return Err(EditorErr::Msg(s))
                            }
                        }
                    }
                    Cmd::EnterNormal=>{
                        enter_normal(focus, nidx, cmd_line);
                    },
                    Cmd::Insert(c)=>{cmd_line.insert(c);},
                    Cmd::BackSpace=>{cmd_line.backspace();},
                    Cmd::MoveLeft=>{cmd_line.cursor = cmd_line.cursor.saturating_sub(1);},
                    Cmd::MoveRight=>{cmd_line.cursor = cmd_line.cursor.saturating_add(1);},
                    Cmd::Open(file)=>{
                        let view = views.get_mut(vidx);
                        view.off = 0;
                        view.cursor = 0;
                        view.prefered_x = 0;
                        let buffer = if let Some(f) = file{
                            if let Some(b) = buffers.get_by_path(&f){
                                let buffer = buffers.get(*b);
                                let line = buffer.buf.char_to_line(buffer.last_cursor);
                                let line_start = buffer.buf.line_to_char(line);
                                let col = buffer.last_cursor - line_start;
                                view.cursor = buffer.last_cursor;
                                view.prefered_x = col;
                                view.off = buffer.last_off;
                                *b
                            }else{
                                buffers.push(Buffer::new(Some(&f), 0)?)
                            }
                        }else{
                            buffers.push(Buffer::new(None, 0)?)
                        };
                        view.buf = buffer;
                        enter_normal(focus, nidx, cmd_line);
                    },
                    Cmd::Close(bidx, force)=>{
                        let view = views.get_mut(vidx);
                        let mut bidx = {
                            if let Some(idx) = bidx{
                                idx
                            }else{
                                view.buf
                            }
                        };
                        let curr_buffer = buffers.get(bidx);
                        if bidx != SCRATCH{
                            if curr_buffer.check_flag(Buffer::READ_ONLY){
                                return Err(EditorErr::ReadOnly(bidx));
                            }
                            if !curr_buffer.undo.is_empty() && force == false{
                                return Err(EditorErr::Dirty(bidx));
                            }else{
                                if view.buf == bidx{
                                    view.buf = SCRATCH;
                                    cmd_line.input.clear();
                                    view.off = 0;
                                    view.cursor = 0;
                                    view.prefered_x = 0;
                                }
                                buffers.remove(&mut bidx);
                            }
                        }else{
                            return Err(EditorErr::Msg("will not close special buffer: 0".into()));
                        }
                        enter_normal(focus, nidx, cmd_line);
                    },
                    Cmd::ViewClose=>{
                        let parent = get_parent(nodes, nidx)?;
                        let Node::Branch {children, focus:f, parent: p, ..} = nodes.data.get_mut(parent.idx).unwrap() else {
                            panic!()
                        };
                        if *p == None && nodes.root.len() == 1 && children.len() == 1{
                            return Err(EditorErr::Msg("cannot close last view".to_string()));
                        }
                        children.remove(*f);
                        if !children.is_empty(){
                            *f = (*f+children.len()-1)%children.len();
                            let mut nidx = *children.get(*f).unwrap();
                            while let Node::Branch {children, focus:f, ..} = nodes.data.get(nidx.idx).unwrap(){
                                nidx = *children.get(*f).unwrap();
                            }
                            *focus = Focus::Node(nidx);
                            recalc(parent, nodes, views);
                        }else{
                            reflow(parent, nodes, views, &mut nidx);
                        }
                        enter_normal(focus, nidx, cmd_line);
                    },
                    Cmd::Save(f)=>{
                        let buffer = buffers.get_mut(bidx);
                        if buffer.check_flag(Buffer::READ_ONLY){
                            // cmd_line.error("cannot save read only");
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
                        enter_normal(focus, nidx, cmd_line);
                    },
                    Cmd::SwitchBuffer(idx)=>{
                        if idx.idx < buffers.len(){
                            if buffers.get(idx).check_flag(Buffer::NON_NAVIGATABLE){
                                return Err(EditorErr::Msg(format!("buffer {} is non navigatable",idx.idx)))?;
                            }
                            let view = views.get_mut(vidx);
                            let buffer = buffers.get_mut(view.buf);
                            buffer.last_off = view.off;
                            buffer.last_cursor = view.cursor;
                            let buffer = buffers.get_mut(idx);
                            if buffer.buf.len_chars() == 0{
                                if let Some(p) = &buffer.file{
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
                            view.scroll(buffer);
                            enter_normal(focus, nidx, cmd_line);
                        }else{
                            return Err(EditorErr::InvalidBuffer);
                        }
                    },
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
                        return Err(EditorErr::Quit)
                    }
                    Cmd::Split=>{
                        let rect = Rect{x:0, y:0, width:0, height:0};
                        let vidx = views.push(View::new(SCRATCH, &[Decoration::StatusBar(rect.clone()), Decoration::LineNumber(rect)]));
                        let parent = get_parent(nodes, nidx)?;
                        nodes.new_leaf(parent, vidx, views);
                        enter_normal(focus, nidx, cmd_line);
                    }
                    Cmd::SplitV=>{
                        let rect = Rect{x:0, y:0, width:0, height:0};
                        let vidx = views.push(View::new(SCRATCH, &[Decoration::StatusBar(rect.clone()), Decoration::LineNumber(rect)]));
                        let parent = get_parent(nodes, nidx)?;
                        nodes.new_branch(parent, vidx, views, Direction::Vertical);
                        enter_normal(focus, nidx, cmd_line);
                    }
                    Cmd::SplitH=>{
                        let rect = Rect{x:0, y:0, width:0, height:0};
                        let vidx = views.push(View::new(SCRATCH, &[Decoration::StatusBar(rect.clone()), Decoration::LineNumber(rect)]));
                        let parent = get_parent(nodes, nidx)?;
                        nodes.new_branch(parent, vidx, views, Direction::Horizontal);
                        enter_normal(focus, nidx, cmd_line);
                    }
                    Cmd::Noop=>{}

                }
                Ok(())
            }
            Ok(())
        }
        Focus::Node(nidx)=>{
            let nidx = nidx.clone();
            let Node::Leaf {vidx, ..} = nodes.data.get(nidx.idx).unwrap() else {panic!()};
            let cmd = key_to_cmd(key, views.get(*vidx));
            exec_cmd(cmd, nodes, focus, nidx, cmd_line, views, buffers)?;
            enum Cmd{
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
                FocusUp,
                FocusDown,
                FocusRight,
                FocusLeft,
                EnterCmd,
                Noop,
            }
            fn key_to_cmd(key: KeyEvent, view: &View)->Cmd{
                match view.mode{
                    Mode::Normal=>{
                        match key.code{
                            KeyCode::Char('i')=>Cmd::EnterInsert,
                            KeyCode::Char(':')=>Cmd::EnterCmd,
                            KeyCode::Char('u')=>Cmd::Undo,
                            KeyCode::Char('U')=>Cmd::Redo,
                            KeyCode::Char('h')=>Cmd::MoveLeft,
                            KeyCode::Char('j')=>Cmd::MoveDown,
                            KeyCode::Char('k')=>Cmd::MoveUp,
                            KeyCode::Char('l')=>Cmd::MoveRight,
                            KeyCode::Char('H')=>Cmd::FocusLeft,
                            KeyCode::Char('J')=>Cmd::FocusDown,
                            KeyCode::Char('K')=>Cmd::FocusUp,
                            KeyCode::Char('L')=>Cmd::FocusRight,
                            _ => Cmd::Noop,
                        }
                    }
                    Mode::Insert=>{
                        match key.code{
                            KeyCode::Esc=>Cmd::EnterNormal,
                            KeyCode::Backspace=>Cmd::BackSpace,
                            KeyCode::Enter=>Cmd::NewLine,
                            KeyCode::Char(c)=>Cmd::Insert(c),
                            _ => Cmd::Noop,
                        }
                    }
                }
            }
            fn exec_cmd(
                cmd: Cmd,
                nodes: &mut Nodes,
                focus: &mut Focus,
                nidx: NodeIdx,
                cmd_line: &mut CmdLine,
                views: &mut Views,
                buffers: &mut Buffers
            )->Result<(), EditorErr>{
                fn enter_normal(focus: &mut Focus, nidx: NodeIdx, view: &mut View, cmd_line: &mut CmdLine){
                    view.mode = Mode::Normal;
                    queue!(stdout(), cursor::SetCursorStyle::SteadyBlock).unwrap();
                    cmd_line.cursor = 0;
                    *focus = Focus::Node(nidx);
                }
    let get_parent = |nidx: NodeIdx| ->Result<NodeIdx, EditorErr> {
        let Node::Leaf {parent, ..} = nodes.data.get(nidx.idx).unwrap() else {return Err(EditorErr::InvalidFocus)};
        Ok(*parent)
    };
                let (bidx, vidx) = {
                let Node::Leaf {vidx, .. } = nodes.data.get(nidx.idx).unwrap() else{panic!()};
                    let view = views.get(*vidx);
                    (view.buf, *vidx)
                };
                match cmd{
                    Cmd::EnterCmd=>{
                        cmd_line.cursor = 0;
                        cmd_line.input.clear();
                        queue!(stdout(), cursor::SetCursorStyle::SteadyBar)?;
                        *focus = Focus::CmdLine;
                    }
                    Cmd::EnterInsert=>{
                        queue!(stdout(), cursor::SetCursorStyle::SteadyBar)?;
                        views.get_mut(vidx).mode = Mode::Insert;
                    }
                    Cmd::EnterNormal=>{
                        enter_normal(focus, nidx, views.get_mut(vidx), cmd_line);
                    }
                    Cmd::FocusUp=>{
                        let parent = get_parent(nidx)?;
                        let Node::Branch {parent, children, focus:f, direction, ..} = nodes.data.get_mut(parent.idx).unwrap()else{panic!()};
                        match direction{
                            Direction::Horizontal=>{
                                *f = (*f+children.len()-1)%children.len();
                                *focus = Focus::Node(*children.get(*f).unwrap());
                            }
                            Direction::Vertical=>{
                                if let Some(p) = parent{
                                    let p = p.clone();
                                    if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap(){
                                        *f = (*f+children.len()-1)%children.len();
                                        *focus = Focus::Node(*children.get(*f).unwrap());
                                    }else{
                                    }
                                }
                            }
                        }
                        let mut nidx = nidx; 
                        while let Node::Branch {children, focus:f, ..} = nodes.data.get(nidx.idx).unwrap(){
                            nidx = *children.get(*f).unwrap();
                        }
                        *focus = Focus::Node(nidx);
                        enter_normal(focus, nidx, views.get_mut(vidx), cmd_line);
                    }
                    Cmd::FocusDown=>{
                        let parent = get_parent(nidx)?;
                        let Node::Branch {parent, children, focus:f, direction, ..} = nodes.data.get_mut(parent.idx).unwrap()else{panic!()};
                        match direction{
                            Direction::Horizontal=>{
                                *f = (*f+1)%children.len();
                                *focus = Focus::Node(*children.get(*f).unwrap());
                            }
                            Direction::Vertical=>{
                                if let Some(p) = parent{
                                    let p = p.clone();
                                    if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap(){
                                        *f = (*f+1)%children.len();
                                        *focus = Focus::Node(*children.get(*f).unwrap());
                                    }else{
                                    }
                                }
                            }
                        }
                        let mut nidx = nidx;
                        while let Node::Branch {children, focus:f, ..} = nodes.data.get(nidx.idx).unwrap(){
                            nidx = *children.get(*f).unwrap();
                        }
                        *focus = Focus::Node(nidx);
                        enter_normal(focus, nidx, views.get_mut(vidx), cmd_line);
                    }
                    Cmd::FocusRight=>{
                        let parent = get_parent(nidx)?;
                        let Node::Branch {parent, children, focus:f, direction, ..} = nodes.data.get_mut(parent.idx).unwrap()else{panic!()};
                        match direction{
                            Direction::Vertical=>{
                                *f = (*f+1)%children.len();
                                *focus = Focus::Node(*children.get(*f).unwrap());
                            }
                            Direction::Horizontal=>{
                                if let Some(p) = parent{
                                    let p = p.clone();
                                    if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap(){
                                        *f = (*f+1)%children.len();
                                        *focus = Focus::Node(*children.get(*f).unwrap());
                                    }else{
                                    }
                                }
                            }
                        }
                        let mut nidx = nidx;
                        while let Node::Branch {children, focus:f, ..} = nodes.data.get(nidx.idx).unwrap(){
                            nidx = *children.get(*f).unwrap();
                        }
                        *focus = Focus::Node(nidx);
                        enter_normal(focus, nidx, views.get_mut(vidx), cmd_line);
                    }
                    Cmd::FocusLeft=>{
                        let parent = get_parent(nidx)?;
                        let Node::Branch {parent, children, focus:f, direction, ..} = nodes.data.get_mut(parent.idx).unwrap()else{panic!()};
                        let mut nidx = nidx;
                        match direction{
                            Direction::Vertical=>{
                                *f = (*f+children.len()-1)%children.len();
                                nidx = *children.get(*f).unwrap();
                            }
                            Direction::Horizontal=>{
                                if let Some(p) = parent{
                                    let p = p.clone();
                                    if let Node::Branch {children, focus: f, ..} = nodes.data.get_mut(p.idx).unwrap(){
                                        *f = (*f+children.len()-1)%children.len();
                                        nidx = *children.get(*f).unwrap();
                                    }else{
                                    }
                                }
                            }
                        }
                        while let Node::Branch {children, focus:f, ..} = nodes.data.get(nidx.idx).unwrap(){
                            nidx = *children.get(*f).unwrap();
                        }
                        *focus = Focus::Node(nidx);
                        enter_normal(focus, nidx, views.get_mut(vidx), cmd_line);
                    }
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
                    }
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
                    }
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
                    }
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
                    }
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
                            return Ok(())
                        }
                        return Err(EditorErr::Msg("undo stack is empty".to_string()))
                    }
                    Cmd::Redo=>{
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
                            let view = views.get_mut(vidx);
                            let buffer = buffers.get_mut(bidx);
                            View::scroll(view, buffer);
                            return Ok(());
                        }
                        return Err(EditorErr::Msg("redo stack is empty".to_string()))
                    }
                    Cmd::Insert(c)=>{
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
                        buffer.insert(view.off, view.cursor, c);
                        let view = views.get_mut(vidx);
                        let line = buffer.buf.char_to_line(view.cursor);
                        let line_start = buffer.buf.line_to_char(line);
                        let col = view.cursor +1 - line_start;

            let line_end = buffer.buf.line(line).len_chars();
            let col = col.min(line_end.saturating_sub(1));

                        view.cursor = line_start + col;
                        view.prefered_x = view.cursor - line_start;
                    }
                    Cmd::NewLine=>{
                        let view = views.get_mut(vidx);
                        let buffer = buffers.get_mut(bidx);
                        buffer.redo.clear();
                        buffer.insert(view.off, view.cursor, '\n');
                        buffer.redo.push(Edit::Insert { idx: view.cursor, text: "\n".to_string() });
                        let line = buffer.buf.char_to_line(view.cursor)+1;
                        let len_lines = buffer.buf.len_lines();
                        let line = line.min(len_lines);
                        let line_start = buffer.buf.line_to_char(line);
                        view.cursor = line_start;
                        View::scroll(view, buffer);
                    }
                    Cmd::BackSpace=>{
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
                    }
                    Cmd::Noop=>{},
                }
                Ok(())
            }
            Ok(())
        }
    }
}

const SCRATCH: BufferIdx = BufferIdx{idx: 0};
fn main()->io::Result<()>{
    let mut views = Views::new();
    let mut buffers = Buffers::new();
    let mut nodes = Nodes{data:vec![], root:vec![], free:vec![]};
    let (width, height) = terminal::size().unwrap();
    let mut cmd_line = CmdLine::new(height-1);
    let mut mode = Mode::Normal;
    let base = nodes.new_root(0, 0, height, width, Direction::Vertical);
    if let Node::Branch {height: h, width: w, ..} = nodes.data.get_mut(base.idx).unwrap(){
        *w = width;
        *h = height-1;
    }
    let mut focus = Focus::Node(base);
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
        let rect = Rect{x:0, y:0, width:0, height:0};
        let vidx = views.push(View::new(bidx, &[Decoration::StatusBar(rect.clone()), Decoration::LineNumber(rect)]));
        focus = Focus::Node(Nodes::new_leaf(&mut nodes, base, vidx, &mut views));
    }
    recalc(base, &mut nodes, &mut views);
    enable_raw_mode()?;
    execute!(stdout(), terminal::EnterAlternateScreen, cursor::SetCursorStyle::SteadyBlock)?;

    //inital draw
    let mut old = ScreenBuffer{cursor_x: 0, cursor_y: 0, width, height, cells: vec![' '; (width * height) as usize]};
    let mut new = ScreenBuffer{cursor_y: 0, cursor_x: 0, width, height, cells: vec![' '; (width * height)as usize]};
    paint(&mode, &mut focus, &mut cmd_line, &nodes, &views, &buffers, &mut old, &mut new)?;
    stdout().flush().unwrap();

    loop{
        if let Event::Key(event) = read()?{
            match key_to_exec(event, &mut nodes, &mut focus, &mut cmd_line, &mut views, &mut buffers){
                Err(e)=> {
                    match e{
                        EditorErr::Msg(msg)=>cmd_line.error(&msg),
                        EditorErr::Dirty(idx)=>cmd_line.error(&format!("buffer:{} is dirty",idx.idx)),
                        EditorErr::InvalidBuffer=>cmd_line.error("index is invalid"),
                        EditorErr::ReadOnly(idx)=>cmd_line.error(&format!("buffer:{}is read only",idx.idx)),
                        EditorErr::InvalidFocus=>cmd_line.error("invalid focus"),
                        EditorErr::Log(msg)=>log(&msg),
                        EditorErr::Io(_)=>{log("IO error"); break},
                        EditorErr::Quit=>break,
                    }
                    let mut n = NodeIdx{idx:0};
                    while let Node::Branch {children, focus:f, ..} = nodes.data.get(n.idx).unwrap(){
                        n = *children.get(*f).unwrap()
                    }
                    let Node::Leaf {vidx, ..} = nodes.data.get(n.idx).unwrap() else {panic!()};
                    let v = views.get_mut(*vidx);
                    v.mode = Mode::Normal;
                    focus = Focus::Node(n);
                    cmd_line.error = false;
                    queue!(stdout(), SetCursorStyle::SteadyBlock)?;
                }
                Ok(_) => {},
            }
            queue!(stdout(), cursor::Hide)?;
            paint(&mode, &mut focus, &mut cmd_line, &nodes, &views, &buffers, &mut old, &mut new)?;
            queue!(stdout(), cursor::Show)?;
        }
        stdout().flush()?;
    }
    disable_raw_mode().unwrap();
    execute!(stdout(), terminal::LeaveAlternateScreen).unwrap();
    Ok(())
}
