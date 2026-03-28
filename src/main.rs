use std::fs::{self, File};
use std::process::{exit};
use ropey::Rope;
use std::path::PathBuf;
use std::{env, io, string};
use std::io::{Write};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::event::Key;

#[derive(Clone, Copy)]
struct Buffer_idx{idx:usize}
struct Buffer{
    flags: u64,
    file: Option<PathBuf>,
    buf: Rope,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            flags: Buffer::EMPTY,
            file: None,
            buf: Rope::new(),
        }
    }
}
impl Buffer{
    const READ_ONLY:       u64 = 1 << 0;
    const SCRATCH:         u64 = 1 << 1;
    const DIRTY:           u64 = 1 << 2;
    const NEW_FILE:        u64 = 1 << 3;
    const EMPTY:           u64 = 1 << 4;
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
            f |= Self::SCRATCH;
            Rope::new()
        };
        Ok(Buffer{
            flags: f,
            buf: buf,
            file: path.map(PathBuf::from),
        })
    }
    fn insert(&mut self, view: &mut View, c: char){
        let cursor_char = view.cursor_char(self);
        self.buf.insert_char(cursor_char, c);
    }
    fn save(&self)->io::Result<()>{
        if let Some(path) = &self.file{
            let file = File::create(path)?;
            self.buf.write_to(file)?;
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
        self.input.insert(self.cursor, c);
        self.cursor += 1;
    }
    fn backspace(&mut self){
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
        }
    }
    fn draw_error(&self, s: &str)->io::Result<()>{
        let mut out = io::stdout().lock();
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
        // if !self.input.is_empty(){
        //     write!(out, "{}{}",termion::cursor::Goto(1, self.pos_y+1), termion::clear::CurrentLine)?;
        //     write!(out, "{}:{}", termion::cursor::Goto(1, self.pos_y+1), self.input)?;
        // }else {
        //     write!(out, "{}{}",termion::cursor::Goto(1, self.pos_y+1), termion::clear::CurrentLine)?;
        // }
        out.flush()
    }
}

struct ViewIdx{idx:usize}
struct View{
    buf: Buffer_idx,
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

impl Default for View{
    fn default() -> Self {
        Self {
            buf: Buffer_idx {idx: 0},
            y: 0,
            x: 0,
            prefered_x: 0,
            off: 0,
            width: 0,
            height: 0,
            pos_x: 0,
            pos_y: 0,
            flags: View::EMPTY,
        }
    }
}
impl View{
    const NON_NAVIGATABLE: u16 = 1 << 0;
    // const FLOATING:        u16 = 1 << 1;
    const LINE_NUMBER:     u16 = 1 << 2;
    const STATUS_BAR:      u16 = 1 << 3;
    const EMPTY:           u16 = 1 << 4;
    fn check_flag(&self, flag: u16)->bool{
        self.flags & flag != 0
    }
    fn new(buf: Buffer_idx, pos_x: u16, pos_y: u16, width: u16, height: u16, flags:u16)->Self{
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
    fn draw(&self, buffers: &Vec<Buffer>, mode: Mode)->io::Result<()>{
        if self.check_flag(View::LINE_NUMBER){
            self.draw_line_numbers()?;
        }else if self.check_flag(View::STATUS_BAR){
            self.draw_status_bar(&buffers[self.buf.idx], mode)?;
        }else{
            self.draw_text(&buffers[self.buf.idx])?;
        }
        Ok(())
    }
    fn draw_status_bar(&self, buffer: &Buffer, mode: Mode)->io::Result<()>{
        let mut out = io::stdout().lock();
        write!(out, "{}", termion::cursor::Goto(self.pos_x+1, self.pos_y+1))?;
        let mut path = "SCRATCH";
        if !buffer.check_flag(Buffer::SCRATCH){
            if let Some(p) = &buffer.file{
                path = p.to_str().unwrap_or("IDK");
            }else{
                path = "NEW_FILE";
            }
        }
        let mode_str = match mode{
            Mode::Command => "CMD",
            Mode::Insert  => "INS",
            _ => "NOR",
        };
        write!(out, "{mode_str} {path}")?;
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
    fn draw_text(&self, buffer: &Buffer) -> io::Result<()>{
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
        buffer.buf.line_to_char(self.y) + self.x
    }
}

struct ViewGroup{
    empty: bool,
    parent: usize,
    children: Vec<usize>,
}

impl Default for ViewGroup{
    fn default() -> Self {
        Self { 
            empty: true,
            parent: 0,
            children: vec![],
        }
    }
}
impl ViewGroup{
    fn new(buffers: &mut Vec<Buffer>, views: &mut Vec<View>, parent_view: usize, children_flags:u16)->Self{
        let parent_pos_x = views[parent_view].pos_x;
        let parent_pos_y = views[parent_view].pos_y;
        let mut parent_height = views[parent_view].height;
        let parent_width = views[parent_view].width;
        let mut children = vec![];
        if children_flags & View::STATUS_BAR != 0 {
            buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
            views.push(View::new(
                Buffer_idx {idx:buffers.len().saturating_sub(1)}, parent_pos_x,
                parent_height - parent_pos_y, parent_width,
                1, View::NON_NAVIGATABLE | View::STATUS_BAR));
            children.push(views.len().saturating_sub(1));
            views[parent_view].height -= 1;
            parent_height -= 1;
        }
        if children_flags & View::LINE_NUMBER != 0 {
            buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
            views.push(View::new(
                Buffer_idx {idx:buffers.len().saturating_sub(1)}, parent_pos_x,
                parent_pos_y, 5,
                parent_height, View::NON_NAVIGATABLE | View::LINE_NUMBER)
            );
            children.push(views.len().saturating_sub(1));
            views[parent_view].pos_x = views[parent_view].pos_x.saturating_add(5);
            views[parent_view].width = views[parent_view].width.saturating_sub(5);
        }
        Self{
            empty: false,
            parent: parent_view,
            children,
        }
    }
        fn sync(&self, views: &mut Vec<View>){
        let (y, off) = {
            let parent = &views[self.parent];
            (parent.y, parent.off)
        };
        for &child in &self.children{
            if !views[child].check_flag(View::STATUS_BAR){
                let child = &mut views[child];
                child.y = y;
                child.off = off;
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
    CmdMoveRight,
    InsertChar(char),
    NewLine,
    Backspace,
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
    Open(String),
    Close,
    Save,
    Quit,
    SwitchNextView,
    SwitchPrevView,
    EnterModeInsert,
    EnterModeNormal,
    EnterModeCommand,
    NoOp,
}

fn key_to_cmd(key: Key, mode: &Mode)->Cmd{
    match key{
        Key::Esc       => Cmd::EnterModeNormal,
        Key::CtrlLeft  => Cmd::SwitchNextView,
        Key::CtrlRight => Cmd::SwitchPrevView,
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

fn exec_cmd(cmd_line: &mut CmdLine, view: usize, views: &mut Vec<View>, buffer: usize, buffers: &mut Vec<Buffer>, groups: &mut Vec<ViewGroup>, cmd: Cmd, mode: &mut Mode){
    let curr_view = &mut views[view];
    let curr_buffer = &mut buffers[buffer];
    match cmd{
        Cmd::EnterModeInsert => {
            *mode = Mode::Insert;
            cmd_line.input.clear();
            cmd_line.draw(*mode).unwrap();
            cmd_line.cursor = 0;
        }
        Cmd::EnterModeNormal  => {
            *mode = Mode::Normal;
            cmd_line.input.clear();
            cmd_line.draw(*mode).unwrap();
            cmd_line.cursor = 0;
        }
        Cmd::EnterModeCommand => {
            *mode = Mode::Command;
            cmd_line.input.clear();
            cmd_line.draw(*mode).unwrap();
        }
        Cmd::InsertChar(c)=>{
            curr_buffer.insert(curr_view, c);
            curr_view.x += 1;
            curr_view.prefered_x = curr_view.x;
            View::scroll(curr_view, curr_buffer);
        },
        Cmd::NewLine=>{
            curr_buffer.insert(curr_view, '\n');
            curr_view.y += 1;
            curr_view.x = 0;
            View::scroll(curr_view, curr_buffer);
        },
        Cmd::Backspace=>{
            let idx = curr_view.cursor_char(curr_buffer);
            if idx != 0{
                curr_buffer.buf.remove(idx - 1..idx);
                if curr_view.x > 0{
                    curr_view.x -= 1;
                    curr_view.prefered_x = curr_view.x;
                }else{
                    curr_view.y = curr_view.y.saturating_sub(1);
                    if let Some(line) = curr_buffer.buf.get_line(curr_view.y){
                        curr_view.x = line.len_chars();
                    }
                }
                View::scroll(curr_view, curr_buffer);
            }
        },
        Cmd::MoveUp=>{
            curr_view.y = curr_view.y.saturating_sub(1);
            if let Some(line) = curr_buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.prefered_x.min(line.len_chars());
            }
            View::scroll(curr_view, curr_buffer);
        },
        Cmd::MoveDown=>{
            if curr_buffer.buf.len_lines() > 0{
                curr_view.y = usize::min(curr_view.y+1, curr_buffer.buf.len_lines().saturating_sub(1));
            }
            if let Some(line) = curr_buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.prefered_x.min(line.len_chars().saturating_sub(1));
            }
            View::scroll(curr_view, curr_buffer);
        },
        Cmd::MoveRight=>{
            curr_view.x = curr_view.x + 1;
            if let Some(line) = curr_buffer.buf.get_line(curr_view.y){
                curr_view.x = curr_view.x.min(line.len_chars().saturating_sub(1));
            }
            curr_view.prefered_x = curr_view.x;

        },
        Cmd::MoveLeft=>{
            curr_view.x = curr_view.x.saturating_sub(1);
            curr_view.prefered_x = curr_view.x;
        },
        Cmd::CmdInsert(c) => {
            if c != '\n' {
                cmd_line.insert(c);
                    cmd_line.draw(*mode).unwrap()
            }else{
                let input = cmd_line.input.clone();
                match parse_cmd(input){
                    Ok(parsed_cmd) => exec_cmd(cmd_line, view, views, buffer, buffers, groups, parsed_cmd, mode),
                    Err(msg) => { 
                        cmd_line.input.clear();
                        cmd_line.cursor = 0;
                        *mode = Mode::Normal;
                        cmd_line.draw_error(&msg).unwrap();
                    },
                }
            }
        },
        Cmd::CmdBackspace=>{
            cmd_line.backspace();
            cmd_line.draw(*mode).unwrap()
        },
        Cmd::CmdMoveLeft=>{
            cmd_line.cursor = cmd_line.cursor.saturating_sub(1);
        }
        Cmd::CmdMoveRight=>{
            cmd_line.cursor = cmd_line.cursor.saturating_add(1);
        }
        Cmd::Save=>{
            match curr_buffer.save(){
                Err(error)=>cmd_line.draw_error(&error.to_string()).unwrap(),
                Ok(_)=>{}
            }
            cmd_line.input.clear();
            cmd_line.cursor = 0;
            *mode = Mode::Normal;
        },
        Cmd::Close=>{
            for g in groups.iter(){
                if views[g.parent].buf.idx == buffer{
                    for &child in &g.children{
                        views[child] = View::default();
                    }
                }
            }
            buffers[buffer] = Buffer::default();
            for g in groups.iter_mut(){
                if views[g.parent].buf.idx == buffer{
                    *g = ViewGroup::default();
                }
            }
            for v in views.iter_mut(){
                if v.buf.idx == buffer{
                    *v = View::default();
                }
            }
        }
        Cmd::Quit=>{
            exit(1);
        },
        Cmd::Open(file_arg) => {
            curr_view.redraw().unwrap();
            if !PathBuf::from(&file_arg).exists(){
                cmd_line.draw_error(&format!("{file_arg} does not exist").to_string()).unwrap();
                *mode = Mode::Normal;
            }else{
                buffers.push(Buffer::new(Some(&file_arg), 0).unwrap());
                curr_view.off = 0;
                curr_view.x = 0;
                curr_view.y = 0;
                curr_view.prefered_x = 0;
                curr_view.buf.idx = buffers.len().saturating_sub(1);
                cmd_line.input.clear();
                cmd_line.cursor = 0;
                *mode = Mode::Normal;
            }
        }
        Cmd::NoOp=>{
        },
        _ =>{}
    }
}

fn parse_cmd(s: String)->Result<Cmd, String>{
    let mut parts = s.trim().split_whitespace();
    let cmd = parts.next().ok_or("empty command")?;
    match cmd{
        "q" => Ok(Cmd::Quit),
        "w" => Ok(Cmd::Save),
        "open" => {
            let file = parts.next().ok_or("missing file name")?;
            Ok(Cmd::Open(file.to_string()))
        }
        "close" => {
            Ok(Cmd::Close)
        }
        _ => Err(format!("unknown command: {}", cmd))
    }
}

fn main()->io::Result<()>{
    let mut views = vec![];
    let mut buffers = vec![];
    let mut groups:Vec<ViewGroup> = Vec::with_capacity(1);
    let (width, height) = termion::terminal_size().unwrap();
    let mut cmd_line = CmdLine::new(height);
    let height = height -2;
    let mut mode = Mode::Normal;
    {
        let args: Vec<String> = env::args().skip(1).collect();
        if args.is_empty(){
            buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
            views.push(View::new(Buffer_idx {idx:buffers.len().saturating_sub(1)},0,0,width,height,0));
            let parent = views.len().saturating_sub(1);
            groups.push(ViewGroup::new(&mut buffers, &mut views, parent, View::LINE_NUMBER | View::STATUS_BAR));
        }else{
            buffers.push(Buffer::new(Some(&args[0]), 0).unwrap());
            views.push(View::new(Buffer_idx{idx:buffers.len().saturating_sub(1)}, 0, 0, width, height, 0));
            let parent = buffers.len().saturating_sub(1);
            groups.push(ViewGroup::new(&mut buffers, &mut views, parent, View::LINE_NUMBER | View::STATUS_BAR));
        }
    }
    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::clear::All)?;

    //inital draw
    for c in &groups[groups.len().saturating_sub(1)].children{
        views[*c].draw(&buffers, mode)?;
    }
    cmd_line.draw(mode)?;
    views[groups[groups.len().saturating_sub(1)].parent].draw(&buffers, mode)?;

    for key in input.keys(){
        let cmd = key_to_cmd(key?, &mode);
        let group = groups.len().saturating_sub(1);
        let view = groups[group].parent;
        let buffer = views[group].buf;
        exec_cmd(&mut cmd_line, view, &mut views, buffer.idx, &mut buffers, &mut groups, cmd, &mut mode);

        match mode {
            Mode::Normal | Mode::Insert => {
                groups[group].sync(&mut views);
                for c in &groups[group].children{
                    views[*c].draw(&buffers, mode)?;
                }
                views[groups[group].parent].draw(&buffers, mode)?;
            }
            Mode::Command =>{
                for c in &groups[group].children{
                    views[*c].draw(&buffers, mode)?;
                }
                cmd_line.draw(mode)?
            } 
        }
    }
    Ok(())
}
