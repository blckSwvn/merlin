pub struct Dummy();
use super::Mode as OtherMode;
use super::*;
mod auto_complete {
    use crossterm::event::KeyModifiers;

    use crate::commandline::cmd_line::COMMAND_REGISTERY;

    use super::*;

    pub struct AutoComplete(pub usize);
    impl AutoComplete {}
    impl Component for AutoComplete {
        fn sketch(
            &self,
            rect: &Rect,
            _views: &Views,
            _buffers: &Buffers,
            _cmd_line: &CmdLine,
            screen: &mut ScreenBuffer,
        ) {
            let rect = sketch_border1(rect, screen);
            let blank = " ".repeat(rect.width as usize);
            for y in 0..rect.height {
                screen.set_string_xy(rect.x, rect.y + y, &blank, FG, BG);
            }
            let mut c = 0;
            let mut offset = 0;
            while c < COMMAND_REGISTERY.len() {
                let mut max = 0;
                for y in 0..rect.height {
                    if c >= COMMAND_REGISTERY.len() {
                        break;
                    }
                    max = max.max(COMMAND_REGISTERY[c].name.chars().count());
                    if c == self.0 {
                        screen.set_string_xy(
                            rect.x + offset,
                            rect.y + y,
                            COMMAND_REGISTERY[c].name,
                            FG,
                            SELECTION,
                        );
                    } else {
                        screen.set_string_xy(
                            rect.x + offset,
                            rect.y + y,
                            COMMAND_REGISTERY[c].name,
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
            rect: &Rect,
            views: &Views,
            buffers: &Buffers,
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
        ) -> Result<(), EditorErr> {
            enum Action {
                Quit,
                Next,
                Prev,
                Complete,
                Exec,
                BackSpace,
                Insert(char),
            }
            let action = match key.code {
                KeyCode::Esc => Action::Quit,
                KeyCode::Char('j') => {
                    if key.modifiers.contains(KeyModifiers::ALT) {
                        Action::Next
                    } else {
                        Action::Insert('j')
                    }
                }
                KeyCode::Char('k') => {
                    if key.modifiers.contains(KeyModifiers::ALT) {
                        Action::Prev
                    } else {
                        Action::Insert('k')
                    }
                }
                KeyCode::Char(' ') => Action::Complete,
                KeyCode::Enter => Action::Exec,
                KeyCode::Char(c) => Action::Insert(c),
                KeyCode::Backspace => Action::BackSpace,
                _ => return Ok(()),
            };
            exec_action(self, action, focus, cmd_line, views, buffers, nodes)?;
            fn exec_action(
                ac: &mut AutoComplete,
                action: Action,
                focus: &mut LeafIdx,
                cmd_line: &mut CmdLine,
                views: &mut Views,
                buffers: &mut Buffers,
                nodes: &mut Nodes,
            ) -> Result<(), EditorErr> {
                match action {
                    Action::BackSpace => cmd_line.backspace(),
                    Action::Quit => {
                        nodes.remove_child(
                            nodes.get_root(ROOT_OVERLAY),
                            views,
                            focus,
                            NodeIdx::Leaf(*focus),
                        );
                        *focus = CMDLINE;
                    }
                    Action::Prev => ac.0 = ac.0.saturating_sub(1),
                    Action::Next => {
                        ac.0 += 1;
                        ac.0 = ac.0.min(COMMAND_REGISTERY.len()-1)
                    }
                    Action::Insert(c) => cmd_line.insert(c),
                    Action::Complete => {
                        let count = cmd_line.input[..cmd_line.cursor]
                            .chars()
                            .rev()
                            .take_while(|&c| c != ' ')
                            .count();
                        for _ in 0..count {
                            cmd_line.backspace();
                        }
                        for c in COMMAND_REGISTERY[ac.0].name.chars() {
                            cmd_line.insert(c);
                        }
                        exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes)?
                    }
                    Action::Exec => {
                        // exec_action(ac, Action::Complete, focus, cmd_line, views, buffers, nodes)?;
                        exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes)?;
                        *focus = CMDLINE;
                        cmd_line.exec(nodes, views, focus, buffers)?;
                    }
                }
                Ok(())
            }
            Ok(())
        }
    }
}
pub mod cmd_line {
    use crate::commandline::auto_complete::AutoComplete;

    use super::auto_complete;
    use super::*;

    enum Mode {
        Normal,
        Insert,
        Visual,
    }

    pub struct CmdLine {
        mode: Mode,
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
        cmd: ParsedWritableCmd,
        cmd_line: &mut CmdLine,
        nodes: &mut Nodes,
        focus: &mut LeafIdx,
        views: &mut Views,
        buffers: &mut Buffers,
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
            Cmd::Close => {
                let view = views.get_mut(vidx);
                let mut bidx = {
                    if let Some(arg) = cmd.argument {
                        match arg {
                            ArgVal::UnsignedNumber(idx) => BufferIdx { idx },
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
                            ArgVal::FilePath(s) => s,
                            _ => return Err(EditorErr::InvalidBuffer),
                        }
                    };
                    if let Some(b) = buffers.get_by_path(&f) {
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
                        ArgVal::UnsignedNumber(num) => num,
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
                nodes.new_leaf(comp, parent, None, (None, None));
                enter_view(focus, lidx, cmd_line);
            }
        }
        Ok(())
    }
    fn parse_cmd(s: &String) -> Result<ParsedWritableCmd, String> {
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
        let mut command: Option<WriteableCmdSpec> = None;
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
                                    ArgKind::UnsignedNumber=>match a.parse::<usize>(){
                                        Ok(num) => Some(ArgVal::UnsignedNumber(num)),
                                        Err(_) => return Err("argument was not provided or was of wrong type or was of wrong type".into()),
                                    },
                                    ArgKind::FilePath => Some(ArgVal::FilePath((*a).into()))
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
        Ok(ParsedWritableCmd {
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
        ) -> Result<(), EditorErr> {
            let cmd = match parse_cmd(&self.input) {
                Ok(o) => o,
                Err(s) => return Err(EditorErr::Msg(s)),
            };
            exec_cmd(cmd, self, nodes, focus, views, buffers)?;
            Ok(())
        }
        pub fn enter_cmd_mode(
            &mut self,
            vidx: ViewIdx,
            focus: &mut LeafIdx,
            views: &mut Views,
            lidx: LeafIdx,
            nodes: &mut Nodes,
        ) {
            self.last_view = (lidx, vidx);
            views.get_mut(vidx).mode = OtherMode::Normal;
            self.input.clear();
            self.cursor = 0;
            *focus = CMDLINE;
            let comp: Box<dyn Component> = Box::new(auto_complete::AutoComplete(0));
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
                rect.x + self.cursor as u16 + 1,
                rect.y,
                match self.mode {
                    Mode::Normal => SetCursorStyle::SteadyBlock,
                    Mode::Visual => SetCursorStyle::SteadyUnderScore,
                    Mode::Insert => SetCursorStyle::SteadyBar,
                },
            )
        }
    }

    enum ArgKind {
        FilePath,
        UnsignedNumber,
    }
    enum ArgVal {
        FilePath(String),
        UnsignedNumber(usize),
    }
    struct ArgSpec {
        kind: ArgKind,
        required: bool,
    }
    pub struct WriteableCmdSpec {
        pub name: &'static str,
        pub arg: Option<ArgSpec>,
        pub forcable: bool,
        pub cmd: Cmd,
    }

    struct ParsedWritableCmd {
        cmd: Cmd,
        argument: Option<ArgVal>,
        force: bool,
    }

    //names are lowercase, kebab case, no spaces, no special charcters
    //implicit aliasing, first letter + every letter directly after - therefore
    //must commands do not collide to cause aliases beeing able to be interpreted in different ways
    ///! suffix is for force, not all commands implement force
    //commands that use arguments need to define if they require an argument or if they are optional argument
    pub const COMMAND_REGISTERY: [WriteableCmdSpec; 10] = [
        WriteableCmdSpec {
            name: "quit",
            arg: None,
            forcable: true,
            cmd: Cmd::Quit,
        },
        WriteableCmdSpec {
            name: "write",
            arg: Some(ArgSpec {
                kind: ArgKind::FilePath,
                required: false,
            }),
            forcable: false,
            cmd: Cmd::Save,
        },
        WriteableCmdSpec {
            name: "open",
            arg: Some(ArgSpec {
                kind: ArgKind::FilePath,
                required: false,
            }),
            forcable: false,
            cmd: Cmd::Open,
        },
        WriteableCmdSpec {
            name: "close",
            arg: Some(ArgSpec {
                kind: ArgKind::UnsignedNumber,
                required: false,
            }),
            forcable: true,
            cmd: Cmd::Close,
        },
        WriteableCmdSpec {
            name: "view-close",
            arg: None,
            forcable: false,
            cmd: Cmd::ViewClose,
        },
        WriteableCmdSpec {
            name: "buffer-switch",
            arg: Some(ArgSpec {
                kind: ArgKind::UnsignedNumber,
                required: true,
            }),
            forcable: false,
            cmd: Cmd::BufferSwitch,
        },
        WriteableCmdSpec {
            name: "buffer-list",
            arg: None,
            forcable: false,
            cmd: Cmd::BufferList,
        },
        WriteableCmdSpec {
            name: "split",
            arg: None,
            forcable: false,
            cmd: Cmd::Split,
        },
        WriteableCmdSpec {
            name: "split-horizontal",
            arg: None,
            forcable: false,
            cmd: Cmd::SplitH,
        },
        WriteableCmdSpec {
            name: "split-vertical",
            arg: None,
            forcable: false,
            cmd: Cmd::SplitV,
        },
    ];

    enum Cmd {
        BufferList,
        Quit,
        Save,
        Open,
        BufferSwitch,
        Close,
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

    fn alias(command: &str) -> String {
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
                rect.x + cmd_line.cursor as u16 + 1,
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
        ) {
            screen.set_string_xy(rect.x, rect.y, &" ".repeat(rect.width as usize), FG, BG);
            let s = {
                if cmd_line.input.is_empty() {
                    return;
                }
                if cmd_line.error {
                    format!("{}", cmd_line.input)
                } else {
                    format!(":{}", cmd_line.input)
                }
            };
            screen.set_string_xy(rect.x, rect.y, &s, FG, BG);
            if let Some(sel) = cmd_line.selection {
                let s = &cmd_line.input[sel.0..sel.1];
                screen.set_string_xy(
                    rect.x + cmd_line.input[..sel.0].chars().count() as u16 + 1,
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
            exec_action(action, cmd_line, nodes, focus, views, buffers)?;

            fn exec_action(
                action: Action,
                cmd_line: &mut CmdLine,
                nodes: &mut Nodes,
                focus: &mut LeafIdx,
                views: &mut Views,
                buffers: &mut Buffers,
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
                        Ok(cmd) => exec_cmd(cmd, cmd_line, nodes, focus, views, buffers)?,
                        Err(s) => return Err(EditorErr::Msg(s)),
                    },
                    Action::EnterView => {
                        cmd_line.selection = None;
                        enter_view(focus, lidx, cmd_line);
                    }
                    Action::Insert(c) => {
                        match c{
                            ' ' => {
                                let comp: Box<dyn Component> = Box::new(AutoComplete(0));
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
                                cmd_line.insert(c);
                            }
                            _ => cmd_line.insert(c),
                        }
                    }
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
                        exec_action(Action::MoveLeft, cmd_line, nodes, focus, views, buffers)
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
                        exec_action(Action::MoveRight, cmd_line, nodes, focus, views, buffers)
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
