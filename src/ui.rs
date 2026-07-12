use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::{queue,
cursor::{MoveTo}
};
use std::io;
use std::io::stdout;
use std::path::PathBuf;
use super::{Component, Views, View, SCRATCH, ROOT_TEXT_VIEW, ROOT_CMD_LINE, ROOT_OVERLAY, Buffers};
use crate::commandline::cmd_line::CmdLine;

pub const FG: Color = Color::White;
pub const BG: Color = Color::Rgb { r: 0, g: 0, b: 0 };
pub const SELECTION: Color = Color::Rgb {
    r: 20,
    g: 140,
    b: 240,
};

#[derive(Clone, Copy)]
pub struct Constraints {
   pub min_height: Constraint,
   pub max_height: Constraint,
   pub min_width: Constraint,
   pub max_width: Constraint,
}

impl Constraints {
    pub fn new()->Self{
        Self{
            min_height: Constraint::Flex,
            max_height: Constraint::Flex,
            min_width: Constraint::Flex,
            max_width: Constraint::Flex
        }
    }
    fn calc_min_width(&self) -> Option<u16> {
        match self.min_width {
            Constraint::Negative(n) => Some(n),
            _ => None,
        }
    }
    fn calc_min_height(&self) -> Option<u16> {
        match self.min_height {
            Constraint::Absolute(a) => Some(a),
            _ => None,
        }
    }
    fn calc_max_width(&self, width: u16) -> u16 {
        match self.max_width {
            Constraint::Flex => width,
            Constraint::Absolute(a) => a,
            Constraint::Relative(r) => width / r,
            Constraint::Negative(n) => width.saturating_sub(n),
        }
    }
    fn calc_max_height(&self, height: u16) -> u16 {
        match self.max_height {
            Constraint::Flex => height,
            Constraint::Absolute(a) => a,
            Constraint::Relative(r) => height / r,
            Constraint::Negative(n) => height.saturating_sub(n),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Constraint {
    Flex,          //default
    Relative(u16), //fraction aka width/x not x%
    Absolute(u16),
    Negative(u16),
}

#[derive(Clone, Copy)]
pub enum Anchor {
    Relative(u16), //fraction aka width/x not x%
    Absolute(u16),
    Negative(u16),
}

#[derive(Clone, Copy)]
pub struct Anchors{
    pub x: Option<Anchor>,
    pub y: Option<Anchor>,
}

impl Anchors{
    pub fn new()->Self{
        Self{x: None, y: None}
    }
}

impl Anchor{
    fn get_enumerated(&self, curr_dimension: u16, parent_dimension: u16) -> u16 {
        match self {
            Anchor::Absolute(a) => {
                if a + curr_dimension > parent_dimension {
                    parent_dimension - curr_dimension
                } else {
                    *a
                }
            }
            Anchor::Negative(n) => parent_dimension.saturating_sub(*n),
            Anchor::Relative(r) => parent_dimension / r - curr_dimension,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub h: u16,
    pub w: u16,
}

#[derive(Clone, Copy, PartialEq)]
pub struct LeafIdx(pub usize);

pub struct Leaf {
    pub parent: SplitIdx,
    pub rect: Rect,
    constraints:Constraints,
    anchors:Anchors,
    pub comp: Box<dyn Component>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum NodeIdx {
    Leaf(LeafIdx),
    Split(SplitIdx),
}

#[derive(Clone, Copy, PartialEq)]
pub struct SplitIdx(pub usize);

pub struct Split {
    pub parent: Option<SplitIdx>,
    pub children: Vec<NodeIdx>,
    pub direction: Direction,
    pub focus: usize,
    pub rect: Rect,
    constraints:Constraints,
    anchors:Anchors,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
}
pub struct Nodes {
    roots: Vec<SplitIdx>,
    splits: Vec<Split>,
    leaves: Vec<Leaf>,
    free_splits: Vec<usize>,
    free_leaves: Vec<usize>,
}
impl Nodes {
    pub fn new() -> Self {
        Nodes {
            roots: vec![],
            splits: vec![],
            leaves: vec![],
            free_splits: vec![],
            free_leaves: vec![],
        }
    }
    pub fn get_root(&self, ridx: usize) -> SplitIdx {
        self.roots[ridx]
    }
    pub fn get_split(&self, sidx: SplitIdx) -> &Split {
        &self.splits[sidx.0]
    }
    pub fn get_mut_split(&mut self, sidx: SplitIdx) -> &mut Split {
        &mut self.splits[sidx.0]
    }
    pub fn get_leaf(&self, lidx: LeafIdx) -> &Leaf {
        &self.leaves[lidx.0]
    }
    pub fn get_mut_leaf(&mut self, lidx: LeafIdx) -> &mut Leaf {
        &mut self.leaves[lidx.0]
    }

    fn push_leaf(&mut self, leaf: Leaf) -> LeafIdx {
        if self.free_leaves.is_empty() {
            let lidx = self.leaves.len();
            self.leaves.push(leaf);
            LeafIdx(lidx)
        } else {
            let lidx = self.free_leaves.pop().unwrap();
            self.leaves[lidx] = leaf;
            LeafIdx(lidx)
        }
    }
    fn push_branch(&mut self, split: Split) -> SplitIdx {
        if self.free_splits.is_empty() {
            let sidx = self.splits.len();
            self.splits.push(split);
            SplitIdx(sidx)
        } else {
            let sidx = self.free_splits.pop().unwrap();
            self.splits[sidx] = split;
            SplitIdx(sidx)
        }
    }

    pub fn new_root(&mut self, constraints:Constraints, anchors:Anchors, direction: Direction) -> SplitIdx {
        let new_root = self.push_branch(Split {
            parent: None,
            children: vec![],
            focus: 0,
            direction,
            rect:Rect{x:0,y:0,w:0,h:0},
            constraints,
            anchors,
        });
        self.roots.push(new_root);
        new_root
    }
    pub fn new_split(
        &mut self,
        comp: Box<dyn Component>,
        parent: SplitIdx,
        direction: Direction,
        constraints: Constraints,
        anchors: Anchors,
    ) -> (LeafIdx, SplitIdx) {
        let new_parent = self.push_branch(Split {
            parent: Some(parent),
            children: vec![],
            focus: 0,
            direction,
            rect: Rect {
                x: 0,
                y: 0,
                h: 0,
                w: 0,
            },
                constraints,
                anchors,
        });
        self.splits[parent.0]
            .children
            .push(NodeIdx::Split(new_parent));
        let lidx = self.new_leaf(comp, new_parent, Constraints::new(), Anchors::new());
        self.recalc(parent);
        (lidx, new_parent)
    }
    pub fn new_leaf(
        &mut self,
        comp: Box<dyn Component>,
        parent: SplitIdx,
        constraints: Constraints,
        anchors: Anchors,
    ) -> LeafIdx {
        let lidx = self.push_leaf(Leaf{
            parent,
            comp,
            rect: Rect {
                x: 0,
                y: 0,
                h: 0,
                w: 0,
            },
            constraints,
            anchors,
        });
        self.splits[parent.0].children.push(NodeIdx::Leaf(lidx));
        self.recalc(parent);
        lidx
    }

    pub fn remove_child(
        &mut self,
        parent: SplitIdx,
        views: &mut Views,
        focus: &mut LeafIdx,
        child: NodeIdx,
    ) {
        let Split {
            children, focus: f, ..
        } = &mut self.splits[parent.0];
        match child {
            NodeIdx::Leaf(lidx) => {
                children.retain(|x| match x {
                    NodeIdx::Leaf(l) => l.0 != lidx.0,
                    _ => true,
                });
            }
            NodeIdx::Split(sidx) => {
                children.retain(|x| match x {
                    NodeIdx::Split(s) => s.0 != sidx.0,
                    _ => true,
                });
            }
        }
        if children.is_empty() {
            *f = 0;
            self.reflow(focus, views, parent);
            let parent = {
                let Leaf { parent, .. } = self.get_leaf(*focus);
                parent
            };
            self.recalc(*parent);
        } else {
            *f = (*f + children.len() - 1) % children.len();
            self.recalc(parent);
        }
        self.remove(child);
        let mut curr = NodeIdx::Split(SplitIdx(0));
        let lidx = loop {
            match curr {
                NodeIdx::Split(s) => {
                    let Split {
                        children, focus: f, ..
                    } = self.get_split(s);
                    curr = *children.get(*f).unwrap();
                }
                NodeIdx::Leaf(l) => break l,
            }
        };
        *focus = lidx;
    }
    fn remove(&mut self, nidx: NodeIdx) {
        match nidx {
            NodeIdx::Leaf(lidx) => {
                self.free_leaves.push(lidx.0);
            }
            NodeIdx::Split(sidx) => {
                self.free_splits.push(sidx.0);
            }
        }
    }

    pub fn recalc_including_root(&mut self, width: u16, height: u16) {
        for ridx in &mut self.roots.clone() {
            let r = self.get_mut_split(*ridx);
            r.rect.h = height;
            r.rect.w = width;
            r.rect.h = r.constraints.calc_max_height(height);
            r.rect.w = r.constraints.calc_max_width(width);
            self.recalc(*ridx);
        }
    }
    pub fn recalc(&mut self, sidx: SplitIdx) {
        let curr = sidx;
        let (children, direction, rect) = {
            let s = self.get_split(curr);
            (s.children.clone(), s.direction.clone(), s.rect.clone())
        };
        if children.is_empty() {
            return;
        }
        let resize: Vec<(u16, NodeIdx)> = {
            let (mut size_left, mut remainder) = {
                match direction {
                    Direction::Vertical => (rect.w, rect.w% children.len() as u16),
                    Direction::Horizontal => (rect.h, rect.h% children.len() as u16),
                }
            };
            let mut resize: Vec<(u16, NodeIdx)> = vec![]; //main axis either width or height
            for n in children.iter() {
                let mut min = 0;
                let (r, c) = match n {
                    NodeIdx::Leaf(l) => {
                        let l = self.get_leaf(*l);
                        (l.rect, l.constraints)
                    }
                    NodeIdx::Split(s) => {
                        self.recalc(*s);
                        (self.get_mut_split(*s).rect, self.get_mut_split(*s).constraints)
                    }
                };
                match direction {
                    Direction::Horizontal => {
                        let m = c.calc_min_height();
                        if let Some(m) = m {
                            min = m;
                            size_left -= m;
                        }
                    }
                    Direction::Vertical => {
                        let m = c.calc_min_width();
                        if let Some(m) = m {
                            min = m;
                            size_left -= m;
                        }
                    }
                }
                resize.push((min, *n));
            }

            let mut non_maxed: Vec<usize> = (0..resize.len()).collect();
            while !non_maxed.is_empty() && size_left != 0 {
                let width_per_child = size_left / non_maxed.len() as u16;
                size_left = 0;
                let mut i = 0;
                while i < non_maxed.len() {
                    let idx = non_maxed[i];
                    let (s, n) = &mut resize[idx];
                    let max = {
                        let (r, c) = match n {
                            NodeIdx::Leaf(l) => (self.get_leaf(*l).rect, self.get_leaf(*l).constraints),
                            NodeIdx::Split(s) => (self.get_split(*s).rect, self.get_split(*s).constraints),
                        };
                        match direction {
                            Direction::Vertical => {
                                c.calc_max_width(rect.w)
                            }
                            Direction::Horizontal => {
                                c.calc_max_height(rect.h)
                            }
                        }
                    };
                    *s += width_per_child;
                    if remainder > 0 {
                        *s += 1;
                        remainder -= 1;
                    }
                    if *s >= max {
                        size_left += s.saturating_sub(max);
                        *s = max;
                        non_maxed.swap_remove(i);
                        continue;
                    }
                    i += 1;
                }
            }
            resize
        };
        let (mut x, mut y) = (rect.x, rect.y);
        let direction = direction.clone();
        let rect = rect.clone();
        for (len, n) in resize {
            let (r, c, a, p_width, p_height) = &mut match n {
                NodeIdx::Leaf(l) => {
                    let curr = self.get_leaf(l);
                    let p = curr.parent;
                    let p_width = self.get_split(p).rect.w;
                    let p_height = self.get_split(p).rect.h;
                    let l = self.get_mut_leaf(l);
                    (&mut l.rect, l.constraints, l.anchors, p_width, p_height)
                }
                NodeIdx::Split(s) => {
                    let curr = self.get_split(s);
                    let p = curr.parent.unwrap();
                    let p_width = self.get_split(p).rect.w;
                    let p_height = self.get_split(p).rect.h;
                    let s = self.get_mut_split(s);
                    (&mut s.rect, s.constraints, s.anchors, p_width, p_height)
                }
            };

            r.x = x;
            r.y = y;
            match direction {
                Direction::Vertical => {
                    r.w = len;
                    x += r.w;
                    r.h = c.calc_max_height(rect.h);
                }
                Direction::Horizontal => {
                    r.h = len;
                    y += r.h;
                    r.w = c.calc_max_width(rect.w);
                }
            }
            if let Some(x) = a.x {
                r.x = x.get_enumerated(r.w, *p_width);
            }

            if let Some(y) = a.y {
                r.y = y.get_enumerated(r.h, *p_height);
            }
            match n {
                NodeIdx::Split(s) => {
                    self.recalc(s);
                }
                _ => {}
            }
        }
    }

    pub fn reflow(&mut self, focus: &mut LeafIdx, views: &mut Views, parent: SplitIdx) {
        let mut to_remove: Option<(SplitIdx, usize, NodeIdx)> = None; //parent, child, node
        let mut curr = parent;
        loop {
            let Split {
                parent, children, ..
            } = self.get_split(curr);
            if children.is_empty() {
                if let Some(p) = parent {
                    let Split { children, .. } = self.get_split(*p);
                    to_remove = Some((
                            *p,
                            children
                            .iter()
                            .position(|x| *x == NodeIdx::Split(curr))
                            .unwrap(),
                            NodeIdx::Split(curr),
                    ));
                    curr = *p
                }
            }
            match to_remove {
                Some(s) => {
                    let Split {
                        children, focus, ..
                    } = &mut self.splits[s.0.0];
                    children.remove(s.1);
                    *focus = focus.saturating_sub(1);
                    self.remove(s.2);
                    to_remove = None;
                }
                None => break,
            }
        }

        //root cannot be empty
        let Split {
            children, focus: f, ..
        } = &mut self.splits[self.roots[ROOT_TEXT_VIEW].0];
        if children.is_empty() {
            let vidx = views.push(View::new(SCRATCH));
            let comp: Box<dyn Component> = Box::new(vidx);
            *f = 0;
            self.new_leaf(comp, self.roots[ROOT_TEXT_VIEW], Constraints::new(), Anchors::new());
        }

        let mut curr = NodeIdx::Split(self.roots[ROOT_TEXT_VIEW]);
        while let NodeIdx::Split(s) = curr {
            let Split {
                children, focus: f, ..
            } = &self.splits[s.0];
            curr = *children.get(*f).unwrap();
        }
        let curr = {
            match curr {
                NodeIdx::Leaf(l) => l,
                _ => panic!(),
            }
        };
        *focus = curr
    }

    pub fn paint(
        &self,
        focus: &LeafIdx,
        cmd_line: &CmdLine,
        views: &Views,
        buffers: &Buffers,
        old: &mut screen::ScreenBuffer,
        new: &mut screen::ScreenBuffer,
        nodes: &Nodes,
        cwd: &PathBuf,
    ) -> io::Result<()> {
        for r in &self.roots {
            sketch(
                &self,
                NodeIdx::Split(*r),
                views,
                buffers,
                cmd_line,
                old,
                new,
                cwd,
                focus,
            );
        }
        new.print(old)?;
        let Leaf { comp, rect, .. } = self.get_leaf(*focus);
        let (x, y, c) = comp
            .cursor_xy(rect, views, buffers, cmd_line, nodes)
            .clone();
        queue!(stdout(), MoveTo(x, y), c)?;
        fn sketch(
            nodes: &Nodes,
            nidx: NodeIdx,
            views: &Views,
            buffers: &Buffers,
            cmd_line: &CmdLine,
            old: &mut screen::ScreenBuffer,
            new: &mut screen::ScreenBuffer,
            cwd: &PathBuf,
            focus: &LeafIdx,
        ) {
            match nidx {
                NodeIdx::Split(s) => {
                    let s = &nodes.splits[s.0];
                    for (i, n) in s.children.iter().enumerate() {
                        if i != s.focus {
                            sketch(nodes, *n, views, buffers, cmd_line, old, new, cwd, focus);
                        }
                    }
                    if let Some(nidx) = s.children.get(s.focus) {
                        sketch(nodes, *nidx, views, buffers, cmd_line, old, new, cwd, focus);
                    }
                }
                NodeIdx::Leaf(l) => {
                    let l = &nodes.leaves[l.0];
                    l.comp.sketch(&l.rect, views, buffers, cmd_line, new, cwd, focus);
                }
            }
        }
        Ok(())
    }

    pub fn focus_right(&mut self, focus: &mut LeafIdx) {
        let l = *focus;
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let x = rect.x + rect.w;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.x >= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.x >= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break l;
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    pub fn focus_left(&mut self, focus: &mut LeafIdx) {
        let l = *focus;
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let x = rect.x;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate().rev() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.x + rect.w <= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.x + rect.w <= x {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break l;
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    pub fn focus_up(&mut self, focus: &mut LeafIdx) {
        let l = *focus;
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let y = rect.y;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate().rev() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.y + rect.h <= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.y + rect.h <= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break l;
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }

    pub fn focus_down(&mut self, focus: &mut LeafIdx) {
        let l = *focus;
        let Leaf { parent, rect, .. } = &self.leaves[l.0];
        let y = rect.y + rect.h;
        let mut curr = *parent;
        let target_split = 'search: loop {
            let Split {
                parent, children, ..
            } = &self.splits[curr.0];
            for (i, c) in children.iter().enumerate() {
                match c {
                    NodeIdx::Leaf(l) => {
                        let Leaf { rect, .. } = &self.leaves[l.0];
                        if rect.y >= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                    NodeIdx::Split(s) => {
                        let Split { rect, .. } = &self.splits[s.0];
                        if rect.y >= y {
                            let c = c.clone();
                            let Split { focus, .. } = &mut self.splits[curr.0];
                            *focus = i;
                            break 'search c;
                        }
                    }
                }
            }
            if let Some(p) = parent {
                curr = *p;
            } else {
                return;
            }
        };
        *focus = {
            let mut curr = target_split;
            loop {
                match curr {
                    NodeIdx::Leaf(l) => {
                        break l;
                    }
                    NodeIdx::Split(s) => {
                        let Split {
                            children, focus: f, ..
                        } = &self.splits[s.0];
                        curr = children[*f];
                    }
                }
            }
        }
    }
}

pub mod screen {
    use super::*;
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Cell {
        pub c: char,
        pub fg: Color,
        pub bg: Color,
    }
    pub struct ScreenBuffer {
        pub cells: Vec<Cell>,
        pub width: u16,
        pub height: u16,
    }
    impl ScreenBuffer {
        pub fn set_cell_xy(&mut self, x: u16, y: u16, cell: Cell) {
            let idx = y * self.width + x;
            self.cells[idx as usize] = cell;
        }
        pub fn set_string_xy(&mut self, x: u16, y: u16, s: &str, fg: Color, bg: Color) {
            for (i, c) in s.chars().enumerate() {
                let xx = x + i as u16;
                if xx >= self.width || y >= self.height {
                    break;
                }
                self.set_cell_xy(xx, y, Cell { c, fg, bg });
            }
        }
        fn clear_buffer(&mut self) {
            self.cells.fill(Cell {
                c: ' ',
                fg: FG,
                bg: BG,
            });
        }
        pub fn print(&mut self, prev: &mut ScreenBuffer) -> io::Result<()> {
            let mut out = stdout().lock();
            let mut current_fg = None;
            let mut current_bg = None;

            for y in 0..self.height {
                let mut x = 0;
                while x < self.width {
                    let idx = (y * self.width + x) as usize;
                    let old = prev.cells[idx];
                    let new = self.cells[idx];
                    if new == old {
                        x += 1;
                        continue;
                    }
                    let start_x = x;
                    let style_fg = new.fg;
                    let style_bg = new.bg;
                    let mut line = String::new();
                    while x < self.width {
                        let idx = (y * self.width + x) as usize;
                        let old = prev.cells[idx];
                        let new = self.cells[idx];
                        //stop if unchganged
                        if new == old {
                            break;
                        }
                        // stop if style changes
                        if new.fg != style_fg || new.bg != style_bg {
                            break;
                        }
                        line.push(new.c);
                        x += 1;
                    }
                    queue!(out, MoveTo(start_x, y))?;
                    if current_fg != Some(style_fg) {
                        queue!(out, SetForegroundColor(style_fg))?;
                        current_fg = Some(style_fg);
                    }
                    if current_bg != Some(style_bg) {
                        queue!(out, SetBackgroundColor(style_bg))?;
                        current_bg = Some(style_bg);
                    }
                    queue!(out, Print(line))?;
                }
            }

            queue!(out, ResetColor)?;

            std::mem::swap(self, prev);
            self.clear_buffer();

            Ok(())
        }
    }
}
