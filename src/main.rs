use std::cell::RefCell;
use std::fs::{self, File};
use ropey::Rope;
use std::rc::Rc;
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
    const NON_NAVIGATABLE: u64 = 1 << 4;
    fn new(path: Option<PathBuf>, flags: Option<u64>)->std::io::Result<Buffer>{
        let mut f = flags.unwrap_or(0);
        let data = if let Some(ref path) = path {
            if path.exists(){
                let cont = fs::read_to_string(path)?;
                if fs::metadata(path)?.permissions().readonly(){
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
            buf: data,
            file: path,
        })
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
}

struct View{
    buf: Rc<RefCell<Buffer>>,
    y: usize,
    x: usize,
    prefered_x: usize,
    off: usize,
}

impl View{
    fn new(buffer: Rc<RefCell<Buffer>>)->Self{
        Self{
            buf: buffer,
            x: 0,
            prefered_x: 0,
            y: 0,
            off: 0,
        }
    }
    
    fn draw(&self) -> io::Result<()> {
        let (_, term_height) = termion::terminal_size()?;
        let mut out = io::stdout().lock();
        write!(out, "{}", termion::clear::All)?;
        write!(out, "{}", termion::cursor::Goto(1, 1))?;

        let height = term_height as usize - 1; // account for 1-indexed terminal
        let start = self.off;
        let buffer = self.buf.borrow();
        let end = usize::min(self.off + height, buffer.buf.len_lines());

        let line_number_width = ((buffer.buf.len_lines() as f32).log10().ceil() as usize).max(1) + 1;

        for i in start..end {
            write!(out, "{:>width$} ", i + 1, width = line_number_width)?;
            write!(out, "{}\r", buffer.buf.line(i))?;
        }

    // draw cursor, offset by line number column
    let cursor_screen_y = self.y.saturating_sub(self.off) + 1;
    write!(
        out,
        "{}",
        termion::cursor::Goto(
            (self.x + line_number_width + 1) as u16,
            cursor_screen_y as u16
        )
    )?;

    out.flush()?;
    Ok(())
}
    fn process_key(&mut self, key: Key){
        match key{
            Key::Char('\n')=> self.new_line(),
            Key::Char(c) => {
                if !c.is_control(){
                    self.insert_char(c);
                }
            }
            Key::Backspace => self.backspace(),
            Key::Up => {
                self.y = self.y.saturating_sub(1);
                if let Some(line) = self.buf.borrow().buf.get_line(self.y){
                    self.x = self.prefered_x.min(line.len_chars());
                }
            }
            Key::Down => {
                if self.buf.borrow().buf.len_lines() > 0{
                    self.y = usize::min(self.y+1, self.buf.borrow().buf.len_lines()-1);
                }
                if let Some(line) = self.buf.borrow().buf.get_line(self.y){
                    self.x = self.prefered_x.min(line.len_chars());
                }
            }
            Key::Left => {
                self.x = self.x.saturating_sub(1);
                self.prefered_x = self.x;
            }
            Key::Right => {
                self.x = self.x + 1;
                if let Some(line) = self.buf.borrow().buf.get_line(self.y){
                    self.x = self.x.min(line.len_chars());
                }
                self.prefered_x = self.x;
            }
            _ => {}
        }

        let (_, height) = termion::terminal_size().unwrap();
        let height = height as usize;
        let buffer = self.buf.borrow();

        if self.y < self.off{
            self.off = self.y;
        } else if self.y >= self.off + height{
            self.off = self.y - height + 1;
        }
        if let Some(line) = buffer.buf.get_line(self.y){
            if line.len_chars() > 0 {
                self.x = usize::min(self.x, line.len_chars());
            }else{
                self.x = 1;
            }
        }else{
            self.x = 1;
        }
    }
    fn insert_char(&mut self, c: char){
        let mut buffer = self.buf.borrow_mut();
        let cursor_char = self.cursor_char(&buffer).saturating_sub(1);
        buffer.buf.insert_char(cursor_char, c);
        self.x += 1;
        self.prefered_x = self.x;
    }
    fn backspace(&mut self){
        let mut buffer = self.buf.borrow_mut();
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
    fn new_line(&mut self){
        let mut buffer = self.buf.borrow_mut();
        let cursor_char = self.cursor_char(&buffer);
        buffer.buf.insert_char(cursor_char, '\n');
        self.y += 1;
        self.x = 0;
    }
    fn cursor_char(&self, buffer: &Buffer) -> usize {
        buffer.buf.line_to_char(self.y) + self.x
    }
    fn save_file(&self)-> io::Result<()>{
        let buffer = self.buf.borrow(); 
        if let Some(path) = &buffer.file{
            let mut file = File::create(path)?;
            buffer.buf.write_to(&mut file)?;
        }
        Ok(())
    }
}

fn main()->io::Result<()>{
    let filename: Option<String> = env::args().nth(1);
    let filename: Option<PathBuf> = filename.map(PathBuf::from);
    let file = Rc::new(RefCell::new(Buffer::new(filename, None)?));
    let mut view = View::new(Rc::clone(&file));
    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::cursor::Show)?;
    view.draw()?;
    for key in input.keys(){
        match key?{
            Key::Ctrl('q')=> break,
            Key::Ctrl('w')=> view.save_file()?,
            Key::Ctrl('x')=> {
                view.save_file()?;
                break
            }
            k => view.process_key(k),
        }
        view.draw()?;
    }
    write!(out,"{}",termion::cursor::Show)?;
    Ok(())
}
