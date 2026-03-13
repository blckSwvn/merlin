use std::fs::{File};
use ropey::Rope;
use std::{env, io};
use std::io::{BufReader, Write};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::event::Key;

struct View{
    buf: Rope,
    y: usize,
    x: usize,
    off: usize,
}

impl View{
    fn init()->Self{
        Self{
            buf: Rope::new(),
            y: 0,
            x: 0,
            off: 0,
        }
    }
    fn open_file(&mut self, filename: &str)->io::Result<()>{
        let file = File::open(filename)?;
        self.buf = Rope::from_reader(BufReader::new(file))?;
        Ok(())
    }
    fn draw(&self) -> io::Result<()> {
        let (_, term_height) = termion::terminal_size()?;
        let mut out = io::stdout().lock();
        write!(out, "{}", termion::clear::All)?;
        write!(out, "{}", termion::cursor::Goto(1, 1))?;

        let height = term_height as usize - 1; // account for 1-indexed terminal
        let start = self.off;
        let end = usize::min(self.off + height, self.buf.len_lines());

        let line_number_width = ((self.buf.len_lines() as f32).log10().ceil() as usize).max(1) + 1;

        for i in start..end {
            let line = self.buf.line(i);
            write!(out, "{:>width$} ", i + 1, width = line_number_width)?;
            // print line content (Ropey lines include '\n')
            write!(out, "{line}\r")?;
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
            }
            Key::Down => {
                if self.buf.len_lines() > 0{
                    self.y = usize::min(self.y+1, self.buf.len_lines()-1);
                }
            }
            Key::Left => {
                self.x = self.x.saturating_sub(1);
            }
            Key::Right => self.x = self.x + 1,
            _ => {}
        }

        let (_, height) = termion::terminal_size().unwrap();
        let height = height as usize;

        if self.y < self.off{
            self.off = self.y;
        } else if self.y >= self.off + height{
            self.off = self.y - height + 1;
        }
        if let Some(line) = self.buf.get_line(self.y){
            if line.len_chars() > 0 {
                self.x = usize::min(self.x, line.len_chars()-1);
            }else{
                self.x = 0;
            }
        }else{
            self.x = 0;
        }
    }
    fn insert_char(&mut self, c: char){
        self.buf.insert_char(self.cursor_char(), c);
        self.x += 1;
    }
    fn backspace(&mut self){
        if self.x != 0 || self.y != 0{
            let idx = self.cursor_char();
            if idx > 0 {
                self.buf.remove(idx - 1..idx);
                if self.x > 0{
                    self.x -= 1;
                }else{
                    self.y -= 1;
                    if let Some(line) = self.buf.get_line(self.y){
                        self.x = line.len_chars();
                    }
                }
            }
        }
    }
    fn new_line(&mut self){
        self.buf.insert_char(self.cursor_char(), '\n');
        self.y += 1;
        self.x = 0;
    }
    fn cursor_char(&self)->usize{
        self.buf.line_to_char(self.y)+self.x
    }
    fn save_file(&self, filename: &str)-> io::Result<()>{
        let mut file = File::create(filename)?;
        self.buf.write_to(&mut file)?;
        Ok(())
    }
}

fn main()->io::Result<()> {
    let filename = env::args().nth(1).expect("invalid filepath");
    let mut editor = View::init();
    editor.open_file(&filename)?;
    let input = io::stdin();
    let mut out = io::stdout().into_raw_mode()?;
    write!(out, "{}",termion::cursor::Show)?;
    editor.draw()?;
    for key in input.keys(){
        match key?{
            Key::Ctrl('q')=> break,
            Key::Ctrl('w')=> editor.save_file(&filename)?,
            Key::Ctrl('x')=> {
                editor.save_file(&filename)?;
                break
            } 
            k => editor.process_key(k),
        }
        editor.draw()?;
    }
    write!(out,"{}",termion::cursor::Show)?;
    Ok(())
}
