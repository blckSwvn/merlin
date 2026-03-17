use std::fs::{self, File};
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
    fn set_flagaa(&mut self, flag: u64){
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
            }
            write!(out, "{}", termion::clear::UntilNewline)?;
        }
        let screen_y = self.pos_y + self.y.saturating_sub(self.off) as u16;
        let screen_x = self.pos_x + self.x as u16;
        write!(out, "{}", termion::cursor::Goto(screen_x+1, screen_y+1))?;
        out.flush()?;
        Ok(())
    }
    fn process_key(&mut self, buffer: &mut Buffer, key: Key){
        match key{
            Key::Char('\n')=> self.new_line(buffer),
            Key::Char(c) => {
                if !c.is_control(){
                    self.insert_char(buffer, c);
                }
            }
            Key::Backspace => self.backspace(buffer),
            Key::Up => {
                self.y = self.y.saturating_sub(1);
                if let Some(line) = buffer.buf.get_line(self.y){
                    self.x = self.prefered_x.min(line.len_chars());
                }
            }
            Key::Down => {
                if buffer.buf.len_lines() > 0{
                    self.y = usize::min(self.y+1, buffer.buf.len_lines().saturating_sub(1));
                }
                if let Some(line) = buffer.buf.get_line(self.y){
                    self.x = self.prefered_x.min(line.len_chars().saturating_sub(1));
                }
            }
            Key::Left => {
                self.x = self.x.saturating_sub(1);
                self.prefered_x = self.x;
            }
            Key::Right => {
                self.x = self.x + 1;
                if let Some(line) = buffer.buf.get_line(self.y){
                    self.x = self.x.min(line.len_chars().saturating_sub(1));
                }
                self.prefered_x = self.x;
            }
            _ => {}
        }

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
    fn insert_char(&mut self, buffer: &mut Buffer, c: char){
        let cursor_char = self.cursor_char(&buffer);
        buffer.buf.insert_char(cursor_char, c);
        self.x += 1;
        self.prefered_x = self.x;
    }
    fn backspace(&mut self, buffer: &mut Buffer){
        if self.x != 0 || self.y != 0{
            let idx = self.cursor_char(&buffer);
            if idx > 0 {
                buffer.buf.remove(idx - 1..idx);
                if self.x > 0{
                    self.x -= 1;
                    self.prefered_x = self.x;
                }else{
                    self.y -= 1;
                    if let Some(line) = buffer.buf.get_line(self.y){
                        self.x = line.len_chars();
                    }
                }
            }
        }
    }
    fn new_line(&mut self, buffer: &mut Buffer){
        let cursor_char = self.cursor_char(&buffer);
        buffer.buf.insert_char(cursor_char, '\n');
        self.y += 1;
        self.x = 0;
    }
    fn cursor_char(&self, buffer: &Buffer) -> usize {
        buffer.buf.line_to_char(self.y) + self.x
    }
    fn save_file(&self, buffer: &Buffer)-> io::Result<()>{
        if let Some(path) = &buffer.file{
            let mut file = File::create(path)?;
            buffer.buf.write_to(&mut file)?;
        }
        Ok(())
    }
}

fn main()->io::Result<()>{
    let mut views = vec![];
    let mut buffers = vec![];
    {
        let (width, height) = termion::terminal_size().unwrap();
        let height = height - 1;//terminal is 1 indexed
        let args: Vec<String> = env::args().skip(1).collect();
        if args.is_empty(){
            buffers.push(Buffer::new(None, 0).unwrap());
            views.push(View::new(buffers.len(),0,0,width,height,0));
        }else{
            let view_count = args.len().max(1);
            let view_width = width / view_count as u16;
            for (i, filename) in args.iter().enumerate(){
                let pos_x = i as u16 * view_width;
                buffers.push(Buffer::new(Some(filename), 0).unwrap());
                views.push(View::new(buffers.len().saturating_sub(1), pos_x, 0, view_width, height, 0));
            }
        }
    }
    let mut active = views.len().saturating_sub(1);

    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::clear::All)?;
    for i in 0..views.len(){
        views[i].draw(&buffers[views[i].buf])?;
    }
    for key in input.keys(){
        let buffer = &mut buffers[views[active].buf];
        match key?{
            Key::Ctrl('q')=> break,
            Key::Ctrl('w')=> views[active].save_file(buffer)?,
            Key::Ctrl('x')=> {
                views[active].save_file(buffer)?;
                break
            }
            Key::CtrlRight=> active = (active.saturating_add(1)) % views.len(),
            k => views[active].process_key(buffer, k),
        }
        views[active].draw(&buffers[views[active].buf])?;
    }
    Ok(())
}

