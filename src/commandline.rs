pub struct Dummy();
use super::Mode as OtherMode;
use super::*;
mod auto_complete {
    use crate::commandline::cmd_line::{ArgKind, COMMAND_REGISTERY, CmdSpec, Mode, alias};
    use cmd_line::CmdLine;
    use crossterm::{event::KeyModifiers, terminal::EnterAlternateScreen};

    use super::*;

    #[derive(Clone)]
    pub struct AutoComplete {
        pub selected: Option<usize>,
        pub filtered: Vec<String>,
        pub progress: Option<&'static CmdSpec>,
    }

    impl AutoComplete {
        pub fn refresh_filtered_and_progress(
            &mut self,
            cmd_line: &CmdLine,
            buffers: &Buffers,
            cwd: &PathBuf,
        ) -> Result<(), ()> {
            let mut parts: Vec<&str> = cmd_line.input.split_whitespace().collect();
            if let Some(p) = parts.get(0) {
                if p.ends_with('!') {
                    parts[0] = &parts[0][..parts[0].len() - 1]; //only safe due to ! beeing ascii
                }
            }
            let progress: Option<&CmdSpec> = if let Some(p) = parts.get(0) {
                COMMAND_REGISTERY
                    .iter()
                    .find(|c| c.name == *p || *p == alias(c.name))
            } else {
                None
            };
            let filtered: Vec<String> = match progress {
                None => {
                    if let Some(p) = parts.get(0) {
                        COMMAND_REGISTERY
                            .iter()
                            .filter(|c| c.name.starts_with(p))
                            .map(|c| c.name.to_string())
                            .collect()
                    } else {
                        COMMAND_REGISTERY
                            .iter()
                            .map(|c| c.name.to_string())
                            .collect()
                    }
                }
                Some(s) => match &s.arg {
                    None => {
                        return Err(());
                    }
                    Some(a) => match a.kind {
                        ArgKind::DirectoryPath => {
                            let entries = fs::read_dir(cwd).map_err(|_| ())?;
                            let mut files: Vec<String> = entries.filter_map(|e| {
                                let e = e.ok()?;
                                if !e.file_type().ok()?.is_dir(){
                                    return None;
                                }
                                let e = e.path();
                                let relative = e.strip_prefix(cwd).ok()?;
                                Some(relative.display().to_string().to_string())
                            }).collect();
                            files = files.into_iter().rev().collect();
                            match parts.get(1){
                                Some(s) => {
                                    files.into_iter().filter(|e| e.starts_with(s)).collect()
                                }
                                None => files,
                            }
                        }
                        ArgKind::FilePath => {
                            use std::fs;

                            let mut files: Vec<String> = vec![];
                            entries(cwd, cwd, &mut files);
                            fn entries(cwd: &PathBuf, dir: &PathBuf, files: &mut Vec::<String>){
                                for entry in fs::read_dir(dir).unwrap() {
                                    let entry = match entry {
                                        Ok(e) => e,
                                        Err(_) => continue,
                                    };

                                    let path = entry.path();

                                    if let Ok(meta) = fs::metadata(&path) {
                                        if meta.is_dir() {
                                            entries(cwd, &path, files);
                                        } else if meta.is_file() {
                                            files.push(path.to_str().unwrap().strip_prefix(cwd.to_str().unwrap()).unwrap().strip_prefix("/").unwrap().to_string());
                                        }
                                    }
                                }
                            }
                            files = files.into_iter().rev().collect();
                            match parts.get(1){
                                Some(s)=>{
                                    files.into_iter().filter(|c| c.starts_with(s)).collect()
                                }
                                None=>files,
                            }
                        }

                        ArgKind::BufferIndex => {
                            (0..buffers.data.len()).map(|i| i.to_string()).collect()
                        }
                    },
                },
            };
            self.filtered = filtered;
            self.progress = progress;
            Ok(())
        }
        pub fn new(cmd_line: &CmdLine, buffers: &Buffers, cwd: &PathBuf) -> Option<AutoComplete> {
            let mut ac = AutoComplete {
                selected: None,
                progress: None,
                filtered: vec![],
            };
            match ac.refresh_filtered_and_progress(cmd_line, buffers, cwd) {
                Ok(()) => Some(ac),
                Err(()) => None,
            }
        }
    }
    impl Component for AutoComplete {
        fn sketch(
            &self,
            rect: &Rect,
            _views: &Views,
            _buffers: &Buffers,
            cmd_line: &CmdLine,
            screen: &mut ScreenBuffer,
            _cwd: &PathBuf,
            _focus: &LeafIdx,
        ) {
            let mut rect = rect.clone();
            let blank = " ".repeat(rect.width as usize);
            for y in 0..rect.height {
                screen.set_string_xy(rect.x, rect.y + y, &blank, FG, BG);
            }
            screen.set_string_xy(rect.x, rect.y, &"─".repeat(rect.width as usize), FG, BG);
            rect.y += 1;
            rect.height -= 1;

            let mut c = self.selected.unwrap_or(0);
            let mut offset = 0u16;

            let parts: Vec<&str> = cmd_line.input.split_whitespace().collect();
            'outer: while c < self.filtered.len() {
                let mut max = 0;
                for y in 0..rect.height {
                    if c >= self.filtered.len() {
                        break;
                    }
                    if self.filtered[c].chars().count() + offset as usize > rect.width as usize{
                        break 'outer;
                    }
                    max = max.max(self.filtered[c].chars().count());
                    if let Some(s) = self.selected {
                        if c == s {
                            screen.set_string_xy(
                                rect.x + offset,
                                rect.y + y,
                                &self.filtered[c],
                                FG,
                                SELECTION,
                            );
                        } else {
                            screen.set_string_xy(
                                rect.x + offset,
                                rect.y + y,
                                &self.filtered[c],
                                FG,
                                BG,
                            );
                        }
                    } else {
                        screen.set_string_xy(
                            rect.x + offset,
                            rect.y + y,
                            &self.filtered[c],
                            FG,
                            BG,
                        );
                    }
                    c += 1;
                }
                offset += max as u16 + 1;
            }
        }
        fn cursor_xy(
            &self,
            _rect: &Rect,
            _views: &Views,
            _buffers: &Buffers,
            cmd_line: &CmdLine,
            nodes: &Nodes,
        ) -> (u16, u16, SetCursorStyle) {
            cmd_line.cursor(&nodes.get_leaf(CMDLINE).rect)
        }
        fn behaviour(
            &mut self,
            key: KeyEvent,
            focus: &mut LeafIdx,
            cmd_line: &mut CmdLine,
            views: &mut Views,
            buffers: &mut Buffers,
            nodes: &mut Nodes,
            cwd: &mut PathBuf,
        ) -> Result<(), EditorErr> {
            enum Action {
                Quit,
                NextCol,
                PrevCol,
                Next,
                Prev,
                Complete,
                BackSpace,
                Exec,
                Insert(char),
            }
            let action = match key.code {
                KeyCode::Esc => Action::Quit,
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::ALT) => Action::Next,
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::ALT) => Action::Prev,
                KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::ALT) => Action::NextCol,
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::ALT) => Action::PrevCol,
                KeyCode::Char(c) => Action::Insert(c),
                KeyCode::Enter => Action::Exec,
                KeyCode::Backspace => Action::BackSpace,
                _ => return Ok(()),
            };
            exec_action(self, action, focus, cmd_line, views, buffers, nodes, cwd)?;
            fn exec_action(
                ac: &mut AutoComplete,
                action: Action,
                focus: &mut LeafIdx,
                cmd_line: &mut CmdLine,
                views: &mut Views,
                buffers: &mut Buffers,
                nodes: &mut Nodes,
                cwd: &mut PathBuf,
            ) -> Result<(), EditorErr> {
                match action {
                    Action::Exec => {
                        let curr = *focus;
                        // *focus = curr;
                        exec_action(
                            ac,
                            Action::Quit,
                            focus,
                            cmd_line,
                            views,
                            buffers,
                            nodes,
                            cwd,
                        ).unwrap();
                        *focus = CMDLINE;
                        let res = cmd_line.exec(nodes, views, focus, buffers, cwd);
                        cmd_line.mode = Mode::Normal;
                        res?
                    }
                    Action::BackSpace => {
                        cmd_line.backspace();
                        ac.selected = None;
                        let _ = ac.refresh_filtered_and_progress(cmd_line, buffers, cwd);
                        if ac.filtered.is_empty() {
                            exec_action(
                                ac,
                                Action::Quit,
                                focus,
                                cmd_line,
                                views,
                                buffers,
                                nodes,
                                cwd,
                            )?;
                        }
                    }
                    Action::Quit => {
                        nodes.remove_child(
                            nodes.get_root(ROOT_OVERLAY),
                            views,
                            focus,
                            NodeIdx::Leaf(*focus),
                        );
                        *focus = CMDLINE;
                    }
                    Action::Prev => {
                        if let Some(s) = ac.selected {
                            ac.selected = Some(s.saturating_sub(1));
                        } else {
                            ac.selected = Some(ac.filtered.len().saturating_sub(1));
                        }
                        if None == ac.filtered.get(ac.selected.unwrap()){
                            ac.selected = None;
                        }
                        exec_action(
                            ac,
                            Action::Complete,
                            focus,
                            cmd_line,
                            views,
                            buffers,
                            nodes,
                            cwd,
                        )?;
                    }
                    Action::Next => {
                        if let Some(s) = ac.selected {
                            ac.selected =
                                Some(usize::min(s + 1, ac.filtered.len().saturating_sub(1)));
                        } else {
                            ac.selected = Some(0);
                        }
                        if None == ac.filtered.get(ac.selected.unwrap()){
                            ac.selected = None;
                        }
                        exec_action(
                            ac,
                            Action::Complete,
                            focus,
                            cmd_line,
                            views,
                            buffers,
                            nodes,
                            cwd,
                        )?;
                    }
                    Action::NextCol=>{
                        let r = nodes.get_leaf(*focus).rect;
                        if let Some(s) = ac.selected {
                            ac.selected =
                                Some(usize::min(s + r.height as usize-1, ac.filtered.len().saturating_sub(1)));
                        } else {
                            ac.selected = Some(usize::min(r.height as usize-1, ac.filtered.len().saturating_sub(1)));
                            if None == ac.filtered.get(ac.selected.unwrap()){
                                ac.selected = None;
                            }
                        }
                        exec_action(
                            ac,
                            Action::Complete,
                            focus,
                            cmd_line,
                            views,
                            buffers,
                            nodes,
                            cwd,
                        )?;
                    }
                    Action::PrevCol=>{
                        let r = nodes.get_leaf(*focus).rect;
                        if let Some(s) = ac.selected {
                            ac.selected =
                                Some(usize::min(s.saturating_sub(r.height as usize-1), ac.filtered.len().saturating_sub(1)));
                        } else {
                            ac.selected = Some(0);
                        }
                        if None == ac.filtered.get(ac.selected.unwrap()){
                            ac.selected = None;
                        }
                        exec_action(
                            ac,
                            Action::Complete,
                            focus,
                            cmd_line,
                            views,
                            buffers,
                            nodes,
                            cwd,
                        )?;

                    }
                    Action::Insert(c) => {
                        if c == ' ' {
                            cmd_line.insert(' ');
                            match ac.refresh_filtered_and_progress(cmd_line, buffers, cwd) {
                                Ok(_) => {}
                                Err(_) => exec_action(
                                    ac,
                                    Action::Quit,
                                    focus,
                                    cmd_line,
                                    views,
                                    buffers,
                                    nodes,
                                    cwd,
                                )
                                .unwrap(),
                            }
                            return Ok(());
                        }
                        cmd_line.insert(c);
                        match ac.refresh_filtered_and_progress(cmd_line, buffers, cwd){
                            Ok(_) => {}
                            Err(_) =>exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes, cwd)?,
                        }
                    }
                    Action::Complete => {
                        let count = cmd_line.input[..cmd_line.cursor]
                            .chars()
                            .rev()
                            .take_while(|&c| c != ' ')
                            .count();
                        for _ in 0..count {
                            cmd_line.backspace();
                        }
                        if let Some(s) = ac.selected {
                            for c in ac.filtered[s].chars() {
                                cmd_line.insert(c);
                            }
                        }
                    }
                }
                Ok(())
            }
            Ok(())
        }
    }
}
//
//
//
pub mod cmd_line {
    use crate::commandline::auto_complete::AutoComplete;

    use super::*;

    pub enum Mode {
        Normal,
        Insert,
        Visual,
    }

    pub struct CmdLine {
        pub mode: Mode,
        pub input: String,
        pub cursor: usize,
        pub error: bool,
        selection: Option<(usize, usize)>,
        last_view: (LeafIdx, ViewIdx),
    }
    fn enter_view(focus: &mut LeafIdx, lidx: LeafIdx, cmd_line: &mut CmdLine) {
        cmd_line.mode = Mode::Insert;
        cmd_line.cursor = 0;
        *focus = lidx;
    }
    fn exec_cmd(
        cmd: CmdVal,
        cmd_line: &mut CmdLine,
        nodes: &mut Nodes,
        focus: &mut LeafIdx,
        views: &mut Views,
        buffers: &mut Buffers,
        cwd: &mut PathBuf,
    ) -> Result<(), EditorErr> {
        let (bidx, vidx, lidx, parent) = {
            let l = nodes.get_leaf(cmd_line.last_view.0);
            (
                views.get(cmd_line.last_view.1).buf,
                cmd_line.last_view.1,
                cmd_line.last_view.0,
                l.parent,
            )
        };
        match cmd.cmd {
            Cmd::Cd => {
                let Some(f) = cmd.argument else { return Ok(()) };
                *cwd = match f {
                    ArgVal::DirectoryPath(s)=>{
                        let mut cwd = cwd.clone();
                        cwd.push(s);
                        match cwd.exists(){
                            true => match cwd.is_dir(){
                                true => {
                                    cwd
                                }
                                false => return Err(EditorErr::Msg("path is not directory".into())),
                            },
                            false => return Err(EditorErr::Msg("path does not exist".into())),
                        }
                    }
                    _ => return Err(EditorErr::Msg("needs to be directory path".into())),
                    };
                exec_cmd(CmdVal {cmd: Cmd::Pwd, argument: None, force: false}, cmd_line, nodes, focus, views, buffers, cwd)?
            },
            Cmd::Pwd => {
                return Err(EditorErr::Msg(format!("{}", cwd.to_str().unwrap())));
            }
            Cmd::Quit => {
                if !cmd.force {
                    let dirty: Vec<_> = buffers
                        .iter()
                        .enumerate()
                        .filter(|(i, b)| !b.undo.is_empty() && *i != SCRATCH.idx)
                        .map(|(i, _)| i)
                        .collect();
                    if !dirty.is_empty() {
                        return Err(EditorErr::Msg(format!(
                            "cant quit dirty buffers: {:?}",
                            dirty
                        )));
                    }
                }
                return Err(EditorErr::Quit);
            }
            Cmd::BufferClose => {
                let view = views.get_mut(vidx);
                let mut bidx = {
                    if let Some(arg) = cmd.argument {
                        match arg {
                            ArgVal::BufferIndex(idx) => BufferIdx { idx },
                            _ => panic!(),
                        }
                    } else {
                        view.buf
                    }
                };
                let curr_buffer = buffers.get(bidx);
                if bidx != SCRATCH {
                    if curr_buffer.check_flag(Buffer::READ_ONLY) {
                        return Err(EditorErr::ReadOnly(bidx));
                    }
                    if !curr_buffer.undo.is_empty() && cmd.force == false {
                        return Err(EditorErr::Dirty(bidx));
                    } else {
                        if view.buf == bidx {
                            view.buf = SCRATCH;
                            cmd_line.input.clear();
                            view.off = 0;
                            view.cursor = 0;
                            view.prefered_x = 0;
                        }
                        buffers.remove(&mut bidx);
                    }
                } else {
                    return Err(EditorErr::Msg("will not close special buffer: 0".into()));
                }
                enter_view(focus, lidx, cmd_line);
            }
            Cmd::Open => {
                let v = views.get_mut(vidx);
                v.off = 0;
                v.cursor = 0;
                v.prefered_x = 0;
                let buffer = if let Some(f) = cmd.argument {
                    let f = {
                        match f {
                            ArgVal::FilePath(s) =>{
                                let mut cwd = cwd.clone();
                                cwd.push(s);
                                cwd
                            } 
                            _ => return Err(EditorErr::InvalidBuffer),
                        }
                    };
                    let f = f.to_str().unwrap();
                    if let Some(b) = buffers.get_by_path(f) {
                        let buffer = buffers.get(*b);
                        let line = buffer.buf.char_to_line(buffer.last_cursor);
                        let line_start = buffer.buf.line_to_char(line);
                        let col = buffer.last_cursor - line_start;
                        v.cursor = buffer.last_cursor;
                        v.prefered_x = col;
                        v.off = buffer.last_off;
                        *b
                    } else {
                        buffers.push(Buffer::new(Some(&f), 0)?)
                    }
                } else {
                    buffers.push(Buffer::new(None, 0)?)
                };
                v.buf = buffer;
                enter_view(focus, lidx, cmd_line);
            }
            Cmd::ViewClose => {
                nodes.remove_child(parent, views, focus, NodeIdx::Leaf(lidx));
                let mut curr = NodeIdx::Split(nodes.get_root(ROOT_TEXT_VIEW));
                let lidx = loop {
                    match curr {
                        NodeIdx::Split(s) => {
                            let Split {
                                children, focus: f, ..
                            } = nodes.get_split(s);
                            curr = *children.get(*f).unwrap();
                        }
                        NodeIdx::Leaf(l) => break l,
                    }
                };
                *focus = lidx;
                enter_view(focus, lidx, cmd_line);
            }
            Cmd::BufferSwitch => {
                let idx = match cmd.argument {
                    Some(a) => match a {
                        ArgVal::BufferIndex(num) => num,
                        _ => panic!(),
                    },
                    None => panic!(),
                };
                let idx = BufferIdx { idx };
                if idx.idx < buffers.len() {
                    if buffers.get(idx).check_flag(Buffer::NON_NAVIGATABLE) {
                        return Err(EditorErr::Msg(format!(
                            "buffer {} is non navigatable",
                            idx.idx
                        )))?;
                    }
                    let view = views.get_mut(vidx);
                    let buffer = buffers.get_mut(view.buf);
                    buffer.last_off = view.off;
                    buffer.last_cursor = view.cursor;
                    let buffer = buffers.get_mut(idx);
                    if buffer.buf.len_chars() == 0 {
                        if let Some(p) = &buffer.file {
                            if Path::new(p).is_file() {
                                let file = File::open(p)?;
                                let reader = BufReader::new(file);
                                buffer.buf = Rope::from_reader(reader)?;
                            }
                        }
                    }
                    view.buf = idx;
                    //prevent panic on cursor > len_chars which happens
                    //if buffer is forcefully closed while cursor is not 0 and buffer is dirty and then revived
                    buffer.last_cursor = buffer.last_cursor.min(buffer.buf.len_chars());
                    buffer.last_off = buffer.last_off.min(buffer.buf.len_chars());
                    view.cursor = buffer.last_cursor;
                    view.off = buffer.last_off;
                    let line = buffer.buf.char_to_line(buffer.last_cursor);
                    let line_start = buffer.buf.line_to_char(line);
                    let col = buffer.last_cursor - line_start;
                    view.cursor = buffer.last_cursor;
                    view.prefered_x = col;
                    view.scroll(&nodes.get_leaf(lidx).rect, buffer);
                    enter_view(focus, lidx, cmd_line);
                } else {
                    return Err(EditorErr::InvalidBuffer);
                }
            }

            Cmd::BufferList => {
                let comp: Box<dyn Component> = Box::new(BufferList {});
                *focus = nodes.new_leaf(
                    comp,
                    nodes.get_root(ROOT_OVERLAY),
                    Some(Constraints {
                        // max_width: Constraint::Flex,
                        max_width: Constraint::Absolute(20),
                        max_height: Constraint::Absolute(buffers.len() as u16 + 2),
                        min_height: Constraint::Flex,
                        min_width: Constraint::Flex,
                    }),
                    (None, None),
                );
            }
            Cmd::Save => {
                let buffer = buffers.get_mut(bidx);
                if buffer.check_flag(Buffer::READ_ONLY) {
                    return Err(EditorErr::ReadOnly(bidx));
                }
                if buffer.check_flag(Buffer::SCRATCH) {
                    return Err(EditorErr::Msg(format!(
                        "cant save, buffer: {} is scratch",
                        bidx.idx
                    )));
                }
                match cmd.argument {
                    Some(a) => {
                        let f = match a {
                            ArgVal::FilePath(p) => p,
                            _ => panic!(),
                        };
                        buffer.save(Some(f))?;
                        buffer.undo.clear();
                        buffer.redo.clear();
                    }
                    None => {
                        if let Some(_) = &buffer.file {
                            match buffer.save(None) {
                                Err(e) => return Err(EditorErr::Io(e)),
                                Ok(_) => {
                                    buffer.undo.clear();
                                    buffer.redo.clear();
                                }
                            }
                        } else {
                            return Err(EditorErr::Msg("new file needs name".into()));
                        }
                    }
                }
                enter_view(focus, lidx, cmd_line);
            }
            Cmd::SplitV => {
                if let Some(idx) = nodes
                    .get_split(parent)
                    .children
                    .iter()
                    .position(|x| *x == NodeIdx::Leaf(lidx))
                {
                    let (l, new_parent) = {
                        let comp: Box<dyn Component> = Box::new(vidx);
                        nodes.new_split(comp, parent, Direction::Vertical, None, (None, None))
                    };
                    let vidx = views.push(View::new(SCRATCH));
                    let comp: Box<dyn Component> = Box::new(vidx);
                    nodes.new_leaf(comp, new_parent, None, (None, None));
                    nodes.get_mut_split(parent).children.swap_remove(idx);
                    enter_view(focus, l, cmd_line);
                    nodes.recalc(parent);
                }
            }
            Cmd::SplitH => {
                if let Some(idx) = nodes
                    .get_split(parent)
                    .children
                    .iter()
                    .position(|x| *x == NodeIdx::Leaf(lidx))
                {
                    let comp: Box<dyn Component> = Box::new(vidx);
                    let (l, new_parent) =
                        nodes.new_split(comp, parent, Direction::Horizontal, None, (None, None));
                    let vidx = views.push(View::new(SCRATCH));
                    let comp: Box<dyn Component> = Box::new(vidx);
                    nodes.new_leaf(comp, new_parent, None, (None, None));
                    nodes.get_mut_split(parent).children.swap_remove(idx);
                    enter_view(focus, l, cmd_line);
                    nodes.recalc(parent);
                }
            }
            Cmd::Split => {
                let vidx = views.push(View::new(SCRATCH));
                let comp: Box<dyn Component> = Box::new(vidx);
                let lidx = nodes.new_leaf(comp, parent, None, (None, None));
                enter_view(focus, lidx, cmd_line);
            }
        }
        Ok(())
    }
    fn parse_cmd(s: &String) -> Result<CmdVal, String> {
        let mut parts: Vec<&str> = s.split_whitespace().collect();
        if parts.is_empty() {
            return Err("command is empty".into());
        }
        let force = {
            if parts[0].ends_with('!') {
                parts[0] = &parts[0][..parts[0].len() - 1]; //only safe due to ! beeing ascii
                true
            } else {
                false
            }
        };
        let mut command: Option<CmdSpec> = None;
        let mut cmd: Option<Cmd> = None;
        let mut argument: Option<ArgVal> = None;
        for c in COMMAND_REGISTERY {
            if c.name == parts[0] || parts[0] == alias(c.name) {
                match force {
                    true => match c.forcable {
                        true => command = Some(c),
                        false => return Err(format!("{} is not forcable", c.name)),
                    },
                    false => command = Some(c),
                }
            }
        }
        if let Some(c) = command {
            cmd = Some(c.cmd);
            argument = {
                if let Some(arg) = c.arg {
                    match parts.get(1){
                            None =>{
                                match arg.required{
                                    true => return Err(format!("command {} requires arg but None was provided",c.name)),
                                    false=> None,
                                }
                            },
                            Some(a)=>{
                                match arg.kind{
                                    ArgKind::BufferIndex=>match a.parse::<usize>(){
                                        Ok(num) => Some(ArgVal::BufferIndex(num)),
                                        Err(_) => return Err("argument was not provided or was of wrong type or was of wrong type".into()),
                                    },
                                    ArgKind::FilePath => Some(ArgVal::FilePath((*a).into())),
                                    ArgKind::DirectoryPath => Some(ArgVal::DirectoryPath((*a).into())),
                                }
                            }
                        }
                } else {
                    None
                }
            };
        } else {
            return Err(format!("{} is not a command nor command alias", parts[0]));
        }
        Ok(CmdVal {
            cmd: cmd.unwrap(),
            argument,
            force,
        })
    }
    impl CmdLine {
        pub fn new() -> Self {
            Self {
                mode: Mode::Insert,
                selection: None,
                input: String::new(),
                cursor: 0,
                error: false,
                last_view: (LeafIdx(usize::MAX), ViewIdx(usize::MAX)),
            }
        }
        pub fn exec(
            &mut self,
            nodes: &mut Nodes,
            views: &mut Views,
            focus: &mut LeafIdx,
            buffers: &mut Buffers,
            cwd: &mut PathBuf,
        ) -> Result<(), EditorErr> {
            let cmd = match parse_cmd(&self.input) {
                Ok(o) => o,
                Err(s) => return Err(EditorErr::Msg(s)),
            };
            exec_cmd(cmd, self, nodes, focus, views, buffers, cwd)?;
            Ok(())
        }
        pub fn enter_cmd_mode(
            &mut self,
            vidx: ViewIdx,
            focus: &mut LeafIdx,
            views: &mut Views,
            lidx: LeafIdx,
            buffers: &Buffers,
            nodes: &mut Nodes,
            cwd: &mut PathBuf,
        ) {
            self.last_view = (lidx, vidx);
            // views.get_mut(vidx).mode = OtherMode::Normal; idk why this was here
            self.input.clear();
            self.cursor = 0;
            self.mode = Mode::Insert;
            *focus = CMDLINE;
            let comp: Box<dyn Component> = Box::new(match AutoComplete::new(self, buffers, cwd) {
                Some(s) => s,
                None => return,
            });
            *focus = nodes.new_leaf(
                comp,
                nodes.get_root(ROOT_OVERLAY),
                Some(Constraints {
                    min_width: Constraint::Flex,
                    max_width: Constraint::Flex,
                    min_height: Constraint::Absolute(7),
                    max_height: Constraint::Absolute(7),
                }),
                (None, Some(Anchor::Negative(8))),
            );
        }

        pub fn insert(&mut self, c: char) {
            if self.error {
                self.cursor = 0;
                self.input.clear();
                self.error = false;
            }
            let byte_idx = self.cursor;
            self.input.insert(byte_idx, c);
            self.cursor += c.len_utf8();
        }
        pub fn backspace(&mut self) {
            if self.error {
                self.cursor = 0;
                self.input.clear();
                self.error = false;
            }
            if self.cursor > 0 {
                let char_len = self.input[..self.cursor]
                    .chars()
                    .rev()
                    .next()
                    .unwrap()
                    .len_utf8();
                self.cursor -= char_len as usize;
                self.input.remove(self.cursor);
            }
        }
        pub fn error(&mut self, s: &str) {
            self.error = true;
            self.input.clear();
            self.input = s.to_string();
        }

        pub fn cursor(&self, rect: &Rect) -> (u16, u16, SetCursorStyle) {
            (
                rect.x + self.cursor as u16,
                rect.y,
                match self.mode {
                    Mode::Normal => SetCursorStyle::SteadyBlock,
                    Mode::Visual => SetCursorStyle::SteadyUnderScore,
                    Mode::Insert => SetCursorStyle::SteadyBar,
                },
            )
        }
    }

    pub enum ArgKind {
        FilePath,
        DirectoryPath,
        BufferIndex,
    }
    enum ArgVal {
        FilePath(String),
        DirectoryPath(String),
        BufferIndex(usize),
    }
    pub struct ArgSpec {
        pub kind: ArgKind,
        pub required: bool,
    }
    pub struct CmdSpec {
        pub name: &'static str,
        pub arg: Option<ArgSpec>,
        pub forcable: bool,
        pub cmd: Cmd,
    }

    struct CmdVal {
        cmd: Cmd,
        argument: Option<ArgVal>,
        force: bool,
    }

    //names are lowercase, kebab case, no spaces, no special charcters
    //implicit aliasing, first letter + every letter directly after - therefore
    //must commands do not collide to cause aliases beeing able to be interpreted in different ways
    ///! suffix is for force, not all commands implement force
    //commands that use arguments need to define if they require an argument or if they are optional argument
    pub const COMMAND_REGISTERY: [CmdSpec; 12] = [
        CmdSpec {
            name: "cd",
            arg: Some(ArgSpec {
                kind: ArgKind::DirectoryPath,
                required: true,
            }),
            forcable: false,
            cmd: Cmd::Cd,
        },
        CmdSpec {
            name: "quit",
            arg: None,
            forcable: true,
            cmd: Cmd::Quit,
        },
        CmdSpec {
            name: "write",
            arg: Some(ArgSpec {
                kind: ArgKind::FilePath,
                required: false,
            }),
            forcable: false,
            cmd: Cmd::Save,
        },
        CmdSpec {
            name: "open",
            arg: Some(ArgSpec {
                kind: ArgKind::FilePath,
                required: false,
            }),
            forcable: false,
            cmd: Cmd::Open,
        },
        CmdSpec {
            name: "buffer-close",
            arg: Some(ArgSpec {
                kind: ArgKind::BufferIndex,
                required: false,
            }),
            forcable: true,
            cmd: Cmd::BufferClose,
        },
        CmdSpec {
            name: "view-close",
            arg: None,
            forcable: false,
            cmd: Cmd::ViewClose,
        },
        CmdSpec {
            name: "buffer-switch",
            arg: Some(ArgSpec {
                kind: ArgKind::BufferIndex,
                required: true,
            }),
            forcable: false,
            cmd: Cmd::BufferSwitch,
        },
        CmdSpec {
            name: "buffer-list",
            arg: None,
            forcable: false,
            cmd: Cmd::BufferList,
        },
        CmdSpec {
            name: "split",
            arg: None,
            forcable: false,
            cmd: Cmd::Split,
        },
        CmdSpec {
            name: "split-horizontal",
            arg: None,
            forcable: false,
            cmd: Cmd::SplitH,
        },
        CmdSpec {
            name: "split-vertical",
            arg: None,
            forcable: false,
            cmd: Cmd::SplitV,
        },
        CmdSpec {
            name: "pwd",
            arg: None,
            forcable: false,
            cmd: Cmd::Pwd,
        },
    ];

    enum Cmd {
        Cd,
        Pwd,
        BufferList,
        Quit,
        Save,
        Open,
        BufferSwitch,
        BufferClose,
        Split,
        SplitV,
        SplitH,
        ViewClose,
    }
    enum Action {
        EnterView,
        EnterNormal,
        EnterVisual,
        EnterInsert,
        Exec,
        Insert(char),
        BackSpace,
        YankClipboard,
        PasteClipboard,
        MoveSelectionLeft,
        MoveSelectionRight,
        MoveLeft,
        MoveRight,
        Noop,
    }

    pub fn alias(command: &str) -> String {
        let mut result = String::new();
        let mut keep_next = true;
        for c in command.chars() {
            if c == '-' {
                keep_next = true;
            } else if keep_next {
                result.push(c);
                keep_next = false;
            }
        }
        result
    }

    pub fn check_alias_collison() {
        if cfg!(debug_assertions) {
            use std::collections::HashMap;
            let mut seen: HashMap<String, &str> = HashMap::new();

            for cmd in COMMAND_REGISTERY {
                let alias = alias(cmd.name);

                if let Some(existing) = seen.insert(alias.clone(), cmd.name) {
                    panic!(
                        "alias collision: '{}' is used by '{}' and '{}'",
                        alias, existing, cmd.name
                    );
                }
            }
        }
    }

    impl Component for Dummy {
        fn cursor_xy(
            &self,
            rect: &Rect,
            _views: &Views,
            _buffers: &Buffers,
            cmd_line: &CmdLine,
            _nodes: &Nodes,
        ) -> (u16, u16, SetCursorStyle) {
            (
                rect.x + cmd_line.cursor as u16,
                rect.y,
                match cmd_line.mode {
                    Mode::Normal => SetCursorStyle::SteadyBlock,
                    Mode::Visual => SetCursorStyle::SteadyUnderScore,
                    Mode::Insert => SetCursorStyle::SteadyBar,
                },
            )
        }
        fn sketch(
            &self,
            rect: &Rect,
            _views: &Views,
            _buffers: &Buffers,
            cmd_line: &CmdLine,
            screen: &mut ScreenBuffer,
            _cwd: &PathBuf,
            _focus: &LeafIdx,
        ) {
            screen.set_string_xy(rect.x, rect.y, &" ".repeat(rect.width as usize), FG, BG);
            let s = {
                if cmd_line.input.is_empty() {
                    return;
                }
                format!("{}", cmd_line.input)
            };
            screen.set_string_xy(rect.x, rect.y, &s, FG, BG);
            if let Some(sel) = cmd_line.selection {
                let s = &cmd_line.input[sel.0..sel.1];
                screen.set_string_xy(
                    rect.x + cmd_line.input[..sel.0].chars().count() as u16,
                    rect.y,
                    s,
                    FG,
                    SELECTION,
                );
            }
        }
        fn behaviour(
            &mut self,
            key: KeyEvent,
            focus: &mut LeafIdx,
            cmd_line: &mut CmdLine,
            views: &mut Views,
            buffers: &mut Buffers,
            nodes: &mut Nodes,
            cwd: &mut PathBuf,
        ) -> Result<(), EditorErr> {
            let action = match cmd_line.mode {
                Mode::Insert => match key.code {
                    KeyCode::Char(c) => Action::Insert(c),
                    KeyCode::Esc => Action::EnterNormal,
                    KeyCode::Backspace => Action::BackSpace,
                    KeyCode::Left => Action::MoveLeft,
                    KeyCode::Right => Action::MoveRight,
                    KeyCode::Enter => Action::Exec,
                    _ => Action::Noop,
                },
                Mode::Visual => match key.code {
                    KeyCode::Esc => Action::EnterNormal,
                    KeyCode::Char('h') => Action::MoveSelectionLeft,
                    KeyCode::Char('y') => Action::YankClipboard,
                    KeyCode::Char('l') => Action::MoveSelectionRight,
                    KeyCode::Char('d') => Action::BackSpace,
                    _ => Action::Noop,
                },
                Mode::Normal => match key.code {
                    KeyCode::Enter => Action::Exec,
                    KeyCode::Esc => Action::EnterView,
                    KeyCode::Char('p') => Action::PasteClipboard,
                    KeyCode::Char('i') => Action::EnterInsert,
                    KeyCode::Char('v') => Action::EnterVisual,
                    KeyCode::Char('h') => Action::MoveLeft,
                    KeyCode::Char('l') => Action::MoveRight,
                    _ => Action::Noop,
                },
            };
            exec_action(action, cmd_line, nodes, focus, views, buffers, cwd)?;

            fn exec_action(
                action: Action,
                cmd_line: &mut CmdLine,
                nodes: &mut Nodes,
                focus: &mut LeafIdx,
                views: &mut Views,
                buffers: &mut Buffers,
                cwd: &mut PathBuf,
            ) -> Result<(), EditorErr> {
                let (bidx, vidx, lidx, parent) = {
                    let l = nodes.get_leaf(cmd_line.last_view.0);
                    (
                        views.get(cmd_line.last_view.1).buf,
                        cmd_line.last_view.1,
                        cmd_line.last_view.0,
                        l.parent,
                    )
                };
                match action {
                    Action::Exec => match parse_cmd(&cmd_line.input) {
                        Ok(cmd) => exec_cmd(cmd, cmd_line, nodes, focus, views, buffers, cwd)?,
                        Err(s) => return Err(EditorErr::Msg(s)),
                    },
                    Action::EnterView => {
                        cmd_line.selection = None;
                        enter_view(focus, lidx, cmd_line);
                    }
                    Action::Insert(c) => match c {
                        ' ' => {
                            cmd_line.insert(c);
                            let comp: Box<dyn Component> =
                                Box::new(match AutoComplete::new(&cmd_line, buffers, cwd) {
                                    Some(s) => s,
                                    None => return Ok(()),
                                });
                            *focus = nodes.new_leaf(
                                comp,
                                nodes.get_root(ROOT_OVERLAY),
                                Some(Constraints {
                                    min_width: Constraint::Flex,
                                    max_width: Constraint::Flex,
                                    min_height: Constraint::Absolute(7),
                                    max_height: Constraint::Absolute(7),
                                }),
                                (None, Some(Anchor::Negative(8))),
                            );
                        }
                        _ => {
                            cmd_line.insert(c);
                            let comp: Box<dyn Component> = Box::new(match AutoComplete::new(&cmd_line, buffers, cwd){
                                Some(s)=> s,
                                None => return Ok(()),
                            });
                            *focus = nodes.new_leaf(comp, nodes.get_root(ROOT_OVERLAY), Some(Constraints{
                                min_width: Constraint::Flex,
                                max_width: Constraint::Flex,
                                min_height: Constraint::Absolute(7),
                                max_height: Constraint::Absolute(7),
                            }), 
                                (None, Some(Anchor::Negative(8))),
                            );
                        }
                    },
                    Action::BackSpace => {
                        if let Some(sel) = cmd_line.selection {
                            cmd_line.cursor = sel.1;
                            for _ in sel.0..sel.1 {
                                cmd_line.backspace();
                            }
                            cmd_line.selection = None;
                            cmd_line.mode = Mode::Normal;
                        } else {
                            cmd_line.backspace();
                        }
                    }
                    Action::YankClipboard => {
                        if let Some(sel) = cmd_line.selection {
                            yank_to_system_clipboard(&cmd_line.input[sel.0..sel.1])?;
                            cmd_line.selection = None;
                            cmd_line.mode = Mode::Normal;
                        }
                    }
                    Action::PasteClipboard => {
                        let mut s = match paste_system_clipboard() {
                            Ok(t) => t,
                            Err(_) => return Ok(()),
                        };
                        s.retain(|c| c != '\n');
                        for c in s.chars() {
                            cmd_line.insert(c);
                        }
                    }
                    Action::MoveSelectionLeft => {
                        exec_action(
                            Action::MoveLeft,
                            cmd_line,
                            nodes,
                            focus,
                            views,
                            buffers,
                            cwd,
                        )
                        .unwrap();
                        let Some(sel) = &mut cmd_line.selection else {
                            return Ok(());
                        };
                        if cmd_line.cursor > sel.0 {
                            sel.1 = cmd_line.cursor;
                        } else {
                            sel.0 = cmd_line.cursor;
                        }
                    }
                    Action::MoveSelectionRight => {
                        exec_action(
                            Action::MoveRight,
                            cmd_line,
                            nodes,
                            focus,
                            views,
                            buffers,
                            cwd,
                        )
                        .unwrap();
                        let Some(sel) = &mut cmd_line.selection else {
                            return Ok(());
                        };
                        //to include the cursor not just until the cursor
                        if cmd_line.cursor >= sel.1 {
                            if cmd_line.cursor == cmd_line.input.len() {
                                return Ok(());
                            }
                            let mut it = cmd_line.input[cmd_line.cursor..].char_indices();
                            it.next();
                            if let Some(it) = it.next() {
                                sel.1 += it.0;
                            } else {
                                sel.1 = cmd_line.input.len();
                            }
                        } else {
                            sel.0 = cmd_line.cursor;
                        }
                    }
                    Action::MoveLeft => {
                        if cmd_line.cursor == 0 {
                            return Ok(());
                        }
                        let mut prev = 0;
                        let prev = {
                            for (i, _) in cmd_line.input.char_indices() {
                                if i >= cmd_line.cursor {
                                    break;
                                }
                                prev = i;
                            }
                            prev
                        };
                        cmd_line.cursor = prev;
                    }
                    Action::MoveRight => {
                        if cmd_line.cursor == cmd_line.input.len() {
                            return Ok(());
                        }
                        let mut it = cmd_line.input[cmd_line.cursor..].char_indices();
                        it.next();
                        if let Some(it) = it.next() {
                            cmd_line.cursor += it.0;
                        } else {
                            cmd_line.cursor = cmd_line.input.len();
                        }
                    }
                    Action::EnterNormal => {
                        cmd_line.selection = None;
                        cmd_line.mode = Mode::Normal;
                    }
                    Action::EnterInsert => {
                        cmd_line.selection = None;
                        cmd_line.mode = Mode::Insert;
                    }
                    Action::EnterVisual => {
                        cmd_line.mode = Mode::Visual;
                        let sel2 = {
                            if cmd_line.cursor == cmd_line.input.len() {
                                cmd_line.cursor
                            } else {
                                let mut it = cmd_line.input[cmd_line.cursor..].char_indices();
                                it.next();
                                if let Some(it) = it.next() {
                                    cmd_line.cursor + it.0
                                } else {
                                    cmd_line.input.len()
                                }
                            }
                        };
                        cmd_line.selection = Some((cmd_line.cursor, sel2))
                    }
                    Action::Noop => {}
                }
                Ok(())
            }
            Ok(())
        }
    }
}
