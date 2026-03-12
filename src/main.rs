use std::fs::{File};
use std::{env, io};
use std::io::{BufRead, BufReader, Write};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::event::Key;

struct View{
    buffer: Vec<String>,
    y: usize,
    x: usize,
    offset: usize,
}

impl View{
    fn init()->Self{
        Self{
            buffer: Vec::new(),
            y: 0,
            x: 0,
            offset: 0,
        }
    }
    fn open_file(&mut self, filename: &str)->io::Result<()>{
        let file = File::open(filename)?;
        self.buffer = BufReader::new(file).lines().collect::<Result<_, _>>()?;
        Ok(())
    }
    fn draw(&self)->io::Result<()>{
        let (_, height) = termion::terminal_size()?;
        let mut out = io::stdout().lock();
        write!(out, "{}",termion::clear::All)?;
        write!(out, "{}", termion::cursor::Goto(1,0))?;

        let height = height as usize;
        let start = self.offset;
        let end = usize::min(self.offset + height, self.buffer.len());
        for line in &self.buffer[start..end] {
            writeln!(out, "{line}\r")?;
        }
        let cursor_screen_y = (self.y - self.offset) + 1;
        write!(
            out,
            "{}",
            termion::cursor::Goto((self.x + 1) as u16, cursor_screen_y as u16)
        )?;
        out.flush()?;
        Ok(())
    }
    fn process_key(&mut self, key: Key){
        match key{
            Key::Up => {
                self.y = self.y.saturating_sub(1);
            }
            Key::Down => {
                if !self.buffer.is_empty(){
                    self.y = usize::min(self.y + 1, self.buffer.len() - 1);
                }
            }
            Key::Left => {
                self.x = self.x.saturating_sub(1);
            }
            Key::Right => {
                    self.x += 1;
            }
            Key::Char('_') => self.x = 0,
            _ => {}
        }
        let (_, height) = termion::terminal_size().unwrap();
        let height = height as usize;

        if self.y < self.offset{
            self.offset = self.y;
        } else if self.y >= self.offset + height {
            self.offset = self.y - height + 1;
        }
        if let Some(line) = self.buffer.get(self.y+1){
            self.x = usize::min(self.x, line.len());
        }else {
            self.x = 0;
        }
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
            Key::Char('q') => break,
            k => editor.process_key(k),
        }
        editor.draw()?;
    }
    write!(out,"{}",termion::cursor::Show)?;
    Ok(())
}
