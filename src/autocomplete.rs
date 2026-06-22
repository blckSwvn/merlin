mod auto_complete{
    use cmd_line::CmdLine;
    use crossterm::{event::KeyModifiers, terminal::EnterAlternateScreen};
    use crate::commandline::cmd_line::{ArgKind, COMMAND_REGISTERY, CmdSpec, Mode, alias};

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
                        ArgKind::DirectoryPath=>{
                            let entries = match parts.get(1) {
                                None => fs::read_dir(cwd)
                                    .map_err(|_| ())?,
                                Some(d) => fs::read_dir(d).map_err(|_| ())?,
                            };
                            entries
                                .filter_map(|e| {
                                    let e = e.ok()?;
                                    if e.file_type().ok()?.is_dir() {
                                        Some(e.path().display().to_string())
                                    } else {
                                        None
                                    }
                                })
                            .collect()
                        }
                        ArgKind::FilePath => {
                            use std::fs;

                            let entries = match parts.get(1) {
                                None => fs::read_dir(cwd)
                                    .map_err(|_| ())?,
                                Some(d) => fs::read_dir(d).map_err(|_| ())?,
                            };
                            entries
                                .filter_map(|e| {
                                    let e = e.ok()?;
                                    let p = e.path();
                                    match fs::metadata(&p) {
                                        Ok(meta) if meta.is_file() => Some(p.display().to_string()),
                                        _ => None,
                                    }
                                })
                            .collect()
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
        ) {
            let mut rect = rect.clone();
            let blank = " ".repeat(rect.width as usize);
            for y in 0..rect.height {
                screen.set_string_xy(rect.x, rect.y + y, &blank, FG, BG);
            }
            screen.set_string_xy(rect.x, rect.y, &"─".repeat(rect.width as usize), FG, BG);
            rect.y += 1;
            rect.height -= 1;

            let mut c = 0;
            let mut offset = 0;

            let parts: Vec<&str> = cmd_line.input.split_whitespace().collect();
            while c < self.filtered.len() {
                let mut max = 0;
                for y in 0..rect.height {
                    if c >= self.filtered.len() {
                        break;
                    }
                    max = max.max(self.filtered[c].chars().count());
                    if let Some(s) = self.selected{
                        if c == s{
                            screen.set_string_xy(
                                rect.x + offset,
                                rect.y + y,
                                &self.filtered[c],
                                FG,
                                SELECTION,
                            );
                        }else{
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
                        // if let Some(p) = parts.get(0) {
                        //     screen.set_string_xy(rect.x + offset, rect.y + y, p, BG, FG);
                        // }
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
                Next,
                Prev,
                Complete,
                BackSpace,
                Exec,
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
                KeyCode::Enter => Action::Exec,
                KeyCode::Char(c) => Action::Insert(c),
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
                        exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes, cwd).unwrap();
                        *focus = CMDLINE;
                        let res = cmd_line.exec(nodes, views, focus, buffers, cwd);
                        cmd_line.mode = Mode::Normal;
                        res?
                    }
                    Action::BackSpace => {
                        cmd_line.backspace();
                        ac.selected = None;
                        let _ = ac.refresh_filtered_and_progress(cmd_line, buffers, cwd);
                        if ac.filtered.is_empty(){
                            exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes, cwd)?;
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
                        if let Some(s) = ac.selected{
                            ac.selected = Some(s.saturating_sub(1));
                        }else{
                            ac.selected = Some(ac.filtered.len().saturating_sub(1));
                        }
                        exec_action(ac, Action::Complete, focus, cmd_line, views, buffers, nodes, cwd)?;
                    }
                    Action::Next => {
                        if let Some(s) = ac.selected{
                            ac.selected = Some(usize::min(s+1, ac.filtered.len().saturating_sub(1)));
                        }else{
                            ac.selected = Some(0);
                        }
                        exec_action(ac, Action::Complete, focus, cmd_line, views, buffers, nodes, cwd)?;
                    }
                    Action::Insert(c) => {
                        if c == ' ' {
                            cmd_line.insert(' ');
                            match ac.refresh_filtered_and_progress(cmd_line, buffers, cwd){
                                Ok(_)=>{}
                                Err(_)=>exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes, cwd).unwrap(),
                            }
                            return Ok(())
                        }
                        cmd_line.insert(c);
                        ac.selected = None;
                        let parts: Vec<&str> = cmd_line.input.split_whitespace().collect();
                        ac.filtered = match parts.get(0) {
                            Some(p) => COMMAND_REGISTERY
                                .iter()
                                .filter(|c| c.name.starts_with(p))
                                .map(|e| e.name.to_string())
                                .collect(),
                            None => COMMAND_REGISTERY
                                .iter()
                                .map(|e| e.name.to_string())
                                .collect(),
                        };
                        if ac.filtered.is_empty(){
                            exec_action(ac, Action::Quit, focus, cmd_line, views, buffers, nodes, cwd)?;
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
                        if let Some(s) = ac.selected{
                            for c in ac.filtered[s].chars(){
                                cmd_line.insert(c);
                            }
                        }
                        // match ac.refresh_filtered_and_progress(cmd_line, buffers, cwd) {
                        //     Err(()) => exec_action(
                        //         ac,
                        //         Action::Quit,
                        //         focus,
                        //         cmd_line,
                        //         views,
                        //         buffers,
                        //         nodes,
                        //         cwd,
                        //     )?,
                        //     Ok(()) => {}
                        // }
                    }
                }
                Ok(())
            }
            Ok(())
        }
    }
}
