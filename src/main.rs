use std::fs::{self, File};
use std::process::exit;
use ropey::Rope;
use std::path::PathBuf;
use std::{env, io};
use std::io::{Write};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::event::Key;

struct Buffer{
    flags: u64,
    file: Option<PathBuf>,
    buf: Rope,
}
impl Buffer{
    const READ_ONLY:       u64 = 1 << 0;
    const SCRATCH:         u64 = 1 << 1;
    const DIRTY:           u64 = 1 << 2;
    const NEW_FILE:        u64 = 1 << 3;
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
    fn remove_char(&mut self, view: &View){
        let idx = view.cursor_char(self);
        self.buf.remove(idx - 1..idx);
    }
}

struct View{
    buf: usize,
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
    const FLOATING:        u16 = 1 << 1;
    const LINE_NUMBER:     u16 = 1 << 2;
    const STATUS_BAR:      u16 = 1 << 3;
    fn check_flag(&self, flag: u16)->bool{
        self.flags & flag != 0
    }
    fn new(buf_index: usize, pos_x: u16, pos_y: u16, width: u16, height: u16, flags:u16)->Self{
        Self{
            buf: buf_index,
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
        write!(out, "{mode_str} {path} {}",self.pos_x)?;
        Ok(())
    }
    fn draw_line_numbers(&self) -> io::Result<()> {
        let mut out = io::stdout().lock();

        let start = self.off;
        let height = self.height as usize;
        let width = self.width as usize;

        for row in 0..height {
            let screen_y = self.pos_y + row as u16 + 1;
            let line_num = start + row + 1;

            write!( out, "{}", termion::cursor::Goto(self.pos_x + 1, screen_y))?;
            write!(out, "{:>width$} ", line_num, width = width.saturating_sub(1))?;
        }
        Ok(())
    }

    fn draw(&self, buffer: &Buffer) -> io::Result<()>{
        let mut out = io::stdout().lock();
        let start = self.off;
        let end = usize::min(start + self.height as usize, buffer.buf.len_lines());

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
            self.off = self.y - self.height as usize + 1;
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
    parent: usize,
    children: Vec<usize>,
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
            views.push(View::new(buffers.len().saturating_sub(1), parent_pos_x, parent_height - parent_pos_y, parent_width, 1, View::NON_NAVIGATABLE | View::STATUS_BAR));
            children.push(views.len().saturating_sub(1));
            views[parent_view].height -= 1;
            parent_height -= 1;
        }
        if children_flags & View::LINE_NUMBER != 0 {
            buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
            views.push(View::new(buffers.len().saturating_sub(1), parent_pos_x, parent_pos_y, 5, parent_height, View::NON_NAVIGATABLE | View::LINE_NUMBER));
            children.push(views.len().saturating_sub(1));
            views[parent_view].pos_x = views[parent_view].pos_x.saturating_add(5);
            views[parent_view].width = views[parent_view].width.saturating_sub(5);
        }
        Self{
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
    InsertChar(char),
    NewLine,
    Backspace,
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
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
        Key::Esc => Cmd::EnterModeNormal,
        Key::CtrlLeft  => Cmd::SwitchNextView,
        Key::CtrlRight => Cmd::SwitchPrevView,
        _ => {
            match mode{
                Mode::Normal=>{
                    match key{
                        Key::Char('i') => Cmd::EnterModeInsert,
                        Key::Char(':') => Cmd::EnterModeCommand,
                        Key::Char('k') => Cmd::MoveUp,
                        Key::Char('j') => Cmd::MoveDown,
                        Key::Char('h') => Cmd::MoveLeft,
                        Key::Char('l') => Cmd::MoveRight,
                        Key::Up    => Cmd::MoveUp,
                        Key::Down  => Cmd::MoveDown,
                        Key::Left  => Cmd::MoveLeft,
                        Key::Right => Cmd::MoveRight,
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
                        Key::Char('w') => Cmd::Save,
                        Key::Char('q') => Cmd::Quit,
                        _ => Cmd::NoOp,
                    }
                },
            }
        }
    }
}

fn exec_cmd(view: &mut View, buffer: &mut Buffer, cmd: Cmd, mode: &mut Mode){
    match cmd{
        Cmd::EnterModeInsert  => *mode = Mode::Insert,
        Cmd::EnterModeNormal  => *mode = Mode::Normal,
        Cmd::EnterModeCommand => *mode = Mode::Command,
        Cmd::InsertChar(c)=>{
            buffer.insert(view, c);
            view.x += 1;
            view.prefered_x = view.x;
            View::scroll(view, buffer);
        },
        Cmd::NewLine=>{
            buffer.insert(view, '\n');
            view.y += 1;
            view.x = 0;
            View::scroll(view, buffer);
        },
        Cmd::Backspace=>{
            if view.x != 0 && view.y != 0 {
                buffer.remove_char(view);
                if view.x > 0{
                    view.x -= 1;
                    view.prefered_x = view.x;
                }
            }else{
                view.y = view.y.saturating_sub(1);
                if let Some(line) = buffer.buf.get_line(view.y){
                    view.x = line.len_chars();
                }
            }
            View::scroll(view, buffer);
        },
        Cmd::MoveUp=>{
            view.y = view.y.saturating_sub(1);
            if let Some(line) = buffer.buf.get_line(view.y){
                view.x = view.prefered_x.min(line.len_chars());
            }
            View::scroll(view, buffer);
        },
        Cmd::MoveDown=>{
            if buffer.buf.len_lines() > 0{
                view.y = usize::min(view.y+1, buffer.buf.len_lines().saturating_sub(1));
            }
            if let Some(line) = buffer.buf.get_line(view.y){
                view.x = view.prefered_x.min(line.len_chars().saturating_sub(1));
            }
            View::scroll(view, buffer);
        },
        Cmd::MoveRight=>{
            view.x = view.x + 1;
            if let Some(line) = buffer.buf.get_line(view.y){
                view.x = view.x.min(line.len_chars().saturating_sub(1));
            }
            view.prefered_x = view.x;

        },
        Cmd::MoveLeft=>{
            view.x = view.x.saturating_sub(1);
            view.prefered_x = view.x;
        },
         Cmd::SwitchNextView=>{
        },
        Cmd::SwitchPrevView=>{
        },
        Cmd::Save=>{
            buffer.save().expect("buffer.save failed");
        },
        Cmd::Quit=>{
            exit(1);
        },
        Cmd::NoOp=>{
        },
    }
}


fn main()->io::Result<()>{
    let mut views = vec![];
    let mut buffers = vec![];
    let mut groups = vec![];
    let mut mode = Mode::Normal;
    let mut cmd_line = 0;
    {
        let (width, height) = termion::terminal_size().unwrap();
        buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
        cmd_line = buffers.len().saturating_sub(1);
        views.push(View::new(cmd_line, 0, 0, height, width, View::NON_NAVIGATABLE));
        let height = height -2;
        let args: Vec<String> = env::args().skip(1).collect();
        if args.is_empty(){
            buffers.push(Buffer::new(None, Buffer::SCRATCH).unwrap());
            views.push(View::new(buffers.len(),0,0,width,height,View::NON_NAVIGATABLE));
            let parent = views.len().saturating_sub(1);
            groups.push(ViewGroup::new(&mut buffers, &mut views, parent, View::LINE_NUMBER | View::STATUS_BAR));
        }else{
            let view_count = args.len().max(1);
            let view_width = width / view_count as u16;
            for (i, filename) in args.iter().enumerate(){
                let pos_x = i as u16 * view_width;
                buffers.push(Buffer::new(Some(filename), 0).unwrap());
                views.push(View::new(buffers.len().saturating_sub(1), pos_x, 0, view_width, height, 0));
                let parent = buffers.len().saturating_sub(1);
                groups.push(ViewGroup::new(&mut buffers, &mut views, parent, View::LINE_NUMBER | View::STATUS_BAR));
            }
        }
    }
    let mut active_group = groups.len().saturating_sub(1);

    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::clear::All)?;
    for i in 0..views.len(){
        if views[i].check_flag(View::LINE_NUMBER){
            views[i].draw_line_numbers()?;
        }else{
            views[i].draw(&buffers[views[i].buf])?;
        }
    }
    for key in input.keys(){
        let mut parent_view = groups[active_group].parent;
        let parent_buffer = views[parent_view].buf;
        let cmd = key_to_cmd(key?, &mode);
        match cmd{
            Cmd::SwitchNextView=>{
                active_group = (active_group.saturating_add(1))%groups.len();
                parent_view = groups[active_group].parent;
            },
            Cmd::SwitchPrevView=>{
                active_group = (active_group.saturating_sub(1))%groups.len();
                parent_view = groups[active_group].parent;
            },
        _ => exec_cmd(&mut views[parent_view], &mut buffers[parent_buffer], cmd, &mut mode),
        }
        for group in &groups{
            group.sync(&mut views);
        }
        for &child_idx in &groups[active_group].children {
            let child_view = &views[child_idx];
            if child_view.check_flag(View::LINE_NUMBER){
                child_view.draw_line_numbers()?;
            } else if child_view.check_flag(View::STATUS_BAR){
                child_view.draw_status_bar(&buffers[views[parent_view].buf], mode)?;
            }
        }
        views[cmd_line].draw(&buffers[views[cmd_line].buf])?;
        views[parent_view].draw(&buffers[views[parent_view].buf])?;
    }
    Ok(())
}
