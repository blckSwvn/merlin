use crossterm::{style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    queue,
    cursor::MoveTo,
};
use ropey::iter::Chars;

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
pub enum Dimension {
    AddAbsolute(u16),
    AddRelative(u16),
    SubAbsolute(u16),
    SubRelative(u16),
}

pub struct Constraints{
   pub min_height: Option<Vec<Dimension>>,
   pub max_height: Option<Vec<Dimension>>,
   pub min_width: Option<Vec<Dimension>>,
   pub max_width: Option<Vec<Dimension>>,
}

impl Constraints {
    pub fn new()->Self{
        Self{
            min_height: None,
            max_height: None,
            min_width: None,
            max_width: None,
        }
    }
    fn calc_min_width(&self, parent:Rect)->Option<u16>{
        let Some(min) = self.min_width.as_ref() else{
            return None
        };
        let mut res = 0;
        for dim in min{
            match *dim{
                Dimension::AddAbsolute(a)=>res+=a,
                Dimension::AddRelative(r)=>res += parent.w/r,
                Dimension::SubAbsolute(a)=> res = res.saturating_sub(a),
                Dimension::SubRelative(r)=> res = res.saturating_sub(parent.w/r),
            }
        }
        res = res.min(parent.w);
        Some(res)
    }
    fn calc_min_height(&self, parent:Rect)->Option<u16>{
        let Some(min) = self.min_height.as_ref() else{
            return None
        };
        let mut res = 0; 
        for dim in min{
            match *dim{
                Dimension::AddAbsolute(a)=>res+=a,
                Dimension::AddRelative(r)=>res += parent.h/r,
                Dimension::SubAbsolute(a)=> res = res.saturating_sub(a),
                Dimension::SubRelative(r)=> res = res.saturating_sub(parent.h/r),
            }
        }
        res = res.min(parent.h);
        Some(res)
    }
    fn calc_max_width(&self, parent:Rect)->Option<u16>{
        let Some(min) = self.max_width.as_ref() else{
            return None
        };
        let mut res = 0;
        for dim in min{
            match *dim{
                Dimension::AddAbsolute(a)=>res+=a,
                Dimension::AddRelative(r)=>res += parent.w/r,
                Dimension::SubAbsolute(a)=> res = res.saturating_sub(a),
                Dimension::SubRelative(r)=> res = res.saturating_sub(parent.w/r),
            }
        }
        res = res.min(parent.w);
        Some(res)
    }
    fn calc_max_height(&self, parent:Rect)->Option<u16>{
        let Some(min) = self.max_height.as_ref() else{
            return None
        };
        let mut res = 0;
        for dim in min{
            match *dim{
                Dimension::AddAbsolute(a)=>res+=a,
                Dimension::AddRelative(r)=>res += parent.h/r,
                Dimension::SubAbsolute(a)=> res = res.saturating_sub(a),
                Dimension::SubRelative(r)=> res = res.saturating_sub(parent.h/r),
            }
        }
        res = res.min(parent.h);
        Some(res)
    }
}

#[derive(Clone, Copy)]
pub enum Position{
    AddAbsolute(u16),
    AddRelative(u16),
    SubAbsolute(u16),
    SubRelative(u16),
}

pub struct Anchors{//relative to parent! x=1 is x = parent.x+1
    pub x: Option<Vec<Position>>,
    pub y: Option<Vec<Position>>,
}

impl Anchors{
    pub fn new()->Self{
        Self{x: None, y: None}
    }
    fn calc_x(&self, parent:Rect)->Option<u16>{
        let Some(x) = self.x.as_ref() else{
            return None;
        };
        let mut res = 0;
        for pos in x{
            match *pos{
                Position::AddAbsolute(a)=>res +=  a,
                Position::AddRelative(r)=>res += parent.w/r,
                Position::SubAbsolute(a)=>res = res.saturating_sub(a),
                Position::SubRelative(r)=> res = res.saturating_sub(parent.w/r),
            }
        }
        Some(res)
    }
    fn calc_y(&self, parent:Rect)->Option<u16>{
        let Some(y) = self.y.as_ref() else{
            return None;
        };
        let mut res = 0;
        for pos in y{
            match *pos{
                Position::AddAbsolute(a)=>res +=  a,
                Position::AddRelative(r)=>res += parent.h/r,
                Position::SubAbsolute(a)=>res = res.saturating_sub(a),
                Position::SubRelative(r)=> res = res.saturating_sub(parent.h/r),
            }
        }
        Some(res)
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

#[derive(Clone, Copy, PartialEq, Eq)]
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

    pub fn new_root(
        &mut self,
        constraints:Constraints,
        anchors:Anchors,
        width: u16,
        height: u16,
        direction: Direction
    ) -> SplitIdx {
        let parent = Rect{x:0, y:0,w:width,h:height};
        let w = constraints.calc_max_width(parent).unwrap_or(width);
        let h = constraints.calc_max_height(parent).unwrap_or(height);
        let new_root = self.push_branch(Split {
            parent: None,
            children: vec![],
            focus: 0,
            direction,
            rect:Rect{x:0,y:0,w,h},
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
            r.rect.w = width;
            r.rect.h = height;
            let parent = Rect{x:0, y:0, w:width, h:height};
            r.rect.h = r.constraints.calc_max_height(parent).unwrap_or(height);
            r.rect.w = r.constraints.calc_max_width(parent).unwrap_or(width);
            // if r.children.is_empty(){return}
            self.recalc(*ridx);
        }
    }
    pub fn recalc(&mut self, sidx: SplitIdx){
        fn get_constraints(nidx: NodeIdx, nodes: &Nodes)->&Constraints{
            match nidx{
                NodeIdx::Leaf(l)=> &nodes.get_leaf(l).constraints,
                NodeIdx::Split(s)=>&nodes.get_split(s).constraints,
            }
        }
        fn get_anchors(nidx: NodeIdx, nodes: &Nodes)->&Anchors{
            match nidx{
                NodeIdx::Split(s)=>&nodes.get_split(s).anchors,
                NodeIdx::Leaf(l)=>&nodes.get_leaf(l).anchors,
            }
        }
        let (children, direction, parent_rect) = {
            let s = self.get_split(sidx);
            (s.children.clone(), s.direction, s.rect)
        };
        let mut space_left = match direction{
            Direction::Vertical => parent_rect.w,
            Direction::Horizontal=>parent_rect.h,
        };
        let mut resize: Vec<(NodeIdx, Rect)> = children.iter().map(|c| (*c, Rect{x:0, y:0, w:0, h:0})).collect();
        for (nidx, rect) in &mut resize{
            let c = get_constraints(*nidx, self);
            match direction{
                Direction::Vertical=>{
                    if let Some(min) = c.calc_min_width(parent_rect){
                        rect.w = min;
                        space_left = space_left.saturating_sub(min);
                    }
                }
                Direction::Horizontal=>{
                    if let Some(min) = c.calc_min_width(parent_rect){
                        rect.h = min;
                        space_left = space_left.saturating_sub(min);
                    }
                }
            }
        }
        let mut active: Vec<usize> = (0..resize.len()).collect();
        while space_left > 0 && !active.is_empty(){
            let space_per = (space_left/active.len() as u16)as u16;
            if space_per == 0{
                break;
            }
            for (active_idx, resize_idx) in active.iter().enumerate(){
                let (nidx, rect) = &mut resize[*resize_idx];
                let c = get_constraints(*nidx, self);
                match direction{
                    Direction::Vertical=>{
                        rect.h = parent_rect.h;
                        if let Some(max) = c.calc_max_width(parent_rect){
                            let add = space_per.min(max.saturating_sub(rect.w));
                            rect.w += add;
                            space_left = space_left.saturating_sub(add);
                            if rect.w >= max{
                                active.remove(active_idx);
                                break;
                            }
                        }else{
                            rect.w += space_per;
                            space_left = space_left.saturating_sub(space_per);
                        }
                    }
                    Direction::Horizontal=>{
                        rect.w = parent_rect.w;
                        if let Some(max) = c.calc_max_height(parent_rect){
                            let add = space_per.min(max.saturating_sub(rect.h));
                            rect.h += add;
                            space_left = space_left.saturating_sub(add);
                            if rect.h >= max{
                                active.remove(active_idx);
                                break
                            }
                        }else{
                            rect.h += space_per;
                            space_left = space_left.saturating_sub(space_per);
                        }
                    }
                }
            }
        }
        let mut x = 0;
        let mut y = 0;
        for (nidx, rect) in &mut resize{
            match direction{
                Direction::Horizontal=>{
                    if let Some(anchor) = get_anchors(*nidx, self).calc_x(parent_rect){
                        rect.x = anchor+parent_rect.x;
                    }else{
                        rect.x = x+parent_rect.x;
                    }
                    if let Some(anchor) = get_anchors(*nidx, self).calc_y(parent_rect){
                        rect.y = anchor+parent_rect.y;
                    }else{
                        rect.y = y+parent_rect.y;
                        y += rect.h;
                    }
                }
                Direction::Vertical=>{
                    if let Some(anchor) = get_anchors(*nidx, self).calc_y(parent_rect){
                        rect.x = anchor+parent_rect.x;
                    }else{
                        rect.x = x+parent_rect.x;
                        x += rect.w;
                    }
                    if let Some(anchor) = get_anchors(*nidx, self).calc_x(parent_rect){
                        rect.y = anchor+parent_rect.y;
                    }else{
                        rect.y = y+parent_rect.y;
                    }
                }
            }
        }
        for (nidx, rect) in &mut resize{
            let r = match nidx{
                NodeIdx::Split(s)=>&mut self.get_mut_split(*s).rect,
                NodeIdx::Leaf(l)=> &mut self.get_mut_leaf(*l).rect,
            };
            *r = *rect;
        }
        for c in children{
            match c{
                NodeIdx::Split(s)=>self.recalc(s),
                _ => {},
            }
        }
    }
    // pub fn recalc(&mut self, sidx: SplitIdx) {
    //     let curr = sidx;
    //     let (children, direction, rect) = {
    //         let s = self.get_split(curr);
    //         (s.children, s.direction, s.rect)
    //     };
    //     if children.is_empty() {
    //         return;
    //     }
    //     let resize: Vec<(u16, NodeIdx)> = {
    //         let (mut size_left, mut remainder) = {
    //             match direction {
    //                 Direction::Vertical => (rect.w, rect.w% children.len() as u16),
    //                 Direction::Horizontal => (rect.h, rect.h% children.len() as u16),
    //             }
    //         };
    //         let mut resize: Vec<(u16, NodeIdx)> = vec![]; //main axis either width or height
    //         for n in children.iter() {
    //             let mut min = 0;
    //             let (r, c) = match n {
    //                 NodeIdx::Leaf(l) => {
    //                     let l = self.get_leaf(*l);
    //                     (l.rect, l.constraints)
    //                 }
    //                 NodeIdx::Split(s) => {
    //                     self.recalc(*s);
    //                     (self.get_mut_split(*s).rect, self.get_mut_split(*s).constraints)
    //                 }
    //             };
    //             match direction {
    //                 Direction::Horizontal => {
    //                     let m = c.calc_min_height(rect);
    //                     if let Some(m) = m {
    //                         min = m;
    //                         size_left -= m;
    //                     }
    //                 }
    //                 Direction::Vertical => {
    //                     let m = c.calc_min_width(rect);
    //                     if let Some(m) = m {
    //                         min = m;
    //                         size_left -= m;
    //                     }
    //                 }
    //             }
    //             resize.push((min, *n));
    //         }
    //
    //         let mut non_maxed: Vec<usize> = (0..resize.len()).collect();
    //         while !non_maxed.is_empty() && size_left != 0 {
    //             let width_per_child = size_left / non_maxed.len() as u16;
    //             size_left = 0;
    //             let mut i = 0;
    //             while i < non_maxed.len() {
    //                 let idx = non_maxed[i];
    //                 let (s, n) = &mut resize[idx];
    //                 let max = {
    //                     let (r, c) = match n {
    //                         NodeIdx::Leaf(l) => (self.get_leaf(*l).rect, self.get_leaf(*l).constraints),
    //                         NodeIdx::Split(s) => (self.get_split(*s).rect, self.get_split(*s).constraints),
    //                     };
    //                     match direction {
    //                         Direction::Vertical => {
    //                             c.calc_max_width(rect)
    //                         }
    //                         Direction::Horizontal => {
    //                             c.calc_max_height(rect)
    //                         }
    //                     }
    //                 };
    //                 *s += width_per_child;
    //                 if remainder > 0 {
    //                     *s += 1;
    //                     remainder -= 1;
    //                 }
    //                 if *s >= max {
    //                     size_left += s.saturating_sub(max);
    //                     *s = max;
    //                     non_maxed.swap_remove(i);
    //                     continue;
    //                 }
    //                 i += 1;
    //             }
    //         }
    //         resize
    //     };
    //     let (mut x, mut y) = (rect.x, rect.y);
    //     let direction = direction.clone();
    //     let rect = rect.clone();
    //     for (len, n) in resize {
    //         let (r, c, a, p_width, p_height) = &mut match n {
    //             NodeIdx::Leaf(l) => {
    //                 let curr = self.get_leaf(l);
    //                 let p = curr.parent;
    //                 let p_width = self.get_split(p).rect.w;
    //                 let p_height = self.get_split(p).rect.h;
    //                 let l = self.get_mut_leaf(l);
    //                 (&mut l.rect, l.constraints, l.anchors, p_width, p_height)
    //             }
    //             NodeIdx::Split(s) => {
    //                 let curr = self.get_split(s);
    //                 let p = curr.parent.unwrap();
    //                 let p_width = self.get_split(p).rect.w;
    //                 let p_height = self.get_split(p).rect.h;
    //                 let s = self.get_mut_split(s);
    //                 (&mut s.rect, s.constraints, s.anchors, p_width, p_height)
    //             }
    //         };
    //
    //         r.x = x;
    //         r.y = y;
    //         match direction {
    //             Direction::Vertical => {
    //                 r.w = len;
    //                 x += r.w;
    //                 r.h = c.calc_max_height(rect).unwrap_or(rect.h);
    //             }
    //             Direction::Horizontal => {
    //                 r.h = len;
    //                 y += r.h;
    //                 r.w = c.calc_max_width(rect).unwrap_or(rect.w);
    //             }
    //         }
    //         if let Some(x) = a.x {
    //             r.x = a.calc_x(rect).unwrap();
    //         }
    //
    //         if let Some(y) = a.y {
    //             r.y = a.calc_y(rect).unwrap();
    //         }
    //         match n {
    //             NodeIdx::Split(s) => {
    //                 self.recalc(s);
    //             }
    //             _ => {}
    //         }
    //     }
    // }

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
