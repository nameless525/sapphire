//! Focus: the one thing a keyboard, a gamepad, a mouse and a finger all move, over plain values, so this
//! crate needs nothing of `sapphire_input` and a test needs no device. `sapphire_controls`' `MenuControls`
//! makes a [`UiInput`] from the input layer's actions and pointers.
//!
//! A direction searches [`UiTree::rect`]s, where layout put things, not where a tween or a scroll has
//! them this frame, so where a press goes doesn't depend on when it comes. A pointer's hit test goes by
//! what was last drawn: placed and masked, last on top.

use sapphire_assets::ui::UiLook;
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;

use super::{UiNodeId, UiTree};
use crate::layout::Rect;

/// How far a pointer goes along a scroller, or across an inset, in points, before it's dragging it and
/// not pressing what's under it.
const DRAG_SLOP: f32 = 8.0;

/// How much room a reveal leaves past a focused node, in points.
const REVEAL_ROOM: f32 = 8.0;

/// A release this long after a drag's last move, in seconds, has stopped first and flings nothing.
const FLICK_GONE: f32 = 0.05;

/// How much a later sample of a drag's speed counts.
const FLICK_SMOOTHING: f32 = 0.5;

/// A direction focus moves in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiStep {
    Up,
    Down,
    Left,
    Right,
}

impl UiStep {
    /// Parses a walk's step.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    /// How far `to` is this way from `from`, or none if it isn't this way: the gap between them, the gap
    /// across (0 if they overlap), and how far apart their centers are across.
    fn distance(self, from: Rect, to: Rect) -> Option<(f32, f32, f32)> {
        let (a, b) = (from.center(), to.center());
        // All of it past this one's middle, so what's beside it isn't that way.
        let (ahead, gap) = match self {
            Self::Up => (to.max.y <= a.y, from.min.y - to.max.y),
            Self::Down => (to.min.y >= a.y, to.min.y - from.max.y),
            Self::Left => (to.max.x <= a.x, from.min.x - to.max.x),
            Self::Right => (to.min.x >= a.x, to.min.x - from.max.x),
        };
        if !ahead {
            return None;
        }
        let (across, centers) = match self {
            Self::Up | Self::Down => ((from.min.x - to.max.x).max(to.min.x - from.max.x), a.x - b.x),
            Self::Left | Self::Right => ((from.min.y - to.max.y).max(to.min.y - from.max.y), a.y - b.y),
        };
        Some((gap.max(0.0), across.max(0.0), centers.abs()))
    }
}

/// What a pointer is, for what hovering means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPointerKind {
    Mouse,
    Touch,
}

/// What a pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPointerPhase {
    Down,
    Move,
    Up,
    /// It's gone, and nothing it was doing happens: the OS took the finger, or focus was lost.
    Cancel,
}

/// One thing a pointer did, in UI points from the frame's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiPointer {
    /// Tells fingers apart.
    pub id: u32,
    pub kind: UiPointerKind,
    pub phase: UiPointerPhase,
    pub position: Vec2,
    pub at: Timestamp,
}

/// What a frame's player asked of a screen, however they asked it.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiInput<'a> {
    pub step: Option<UiStep>,
    pub confirm: bool,
    pub back: bool,
    /// Points to go through what scrolls, +y further down it.
    pub scroll: Vec2,
    pub pointers: &'a [UiPointer],
}

/// What happened on a screen, for its game to act on: see [`UiTree::drain_events`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// A node was chosen: a press of confirm while focused, or a pointer let go on it.
    Activated(UiNodeId),
    /// Focus went somewhere else.
    FocusMoved { from: Option<UiNodeId>, to: Option<UiNodeId> },
    /// The player asked to go back. The machine has seen it too, as the event `back`.
    Back,
}

/// What's under a point, as a hit test finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiHit {
    /// Nothing: it belongs to whatever is behind the screen.
    Nothing,
    /// Something that shows, which takes the pointer and does nothing with it.
    Surface,
    /// Something that takes focus, or something on it.
    Focusable(UiNodeId),
}

/// A pointer held down.
#[derive(Clone, Copy, Debug)]
pub(super) struct Hold {
    id: u32,
    kind: UiPointerKind,
    start: Vec2,
    last: Vec2,
    last_at: Timestamp,
    /// Along the scroller, in points a second.
    velocity: f32,
    /// What it pressed.
    node: Option<u32>,
    scroller: Option<u32>,
    start_offset: f32,
    /// The inset it turns, how far that was turned when it started, and where across it started turning.
    inset: Option<(u32, f32, f32)>,
    dragging: bool,
}

impl UiTree {
    /// Takes a frame's input. Call it before [`Self::update`], which draws what it did. A hit test goes
    /// by the last frame's picture, the one the player was looking at.
    pub fn input(&mut self, input: &UiInput<'_>, now: Timestamp) {
        profiling::scope!("UiTree::input");
        self.player.advance(now, &mut self.nodes);
        let open = self.player.accepts_input();
        for &pointer in input.pointers {
            self.pointer(pointer, open);
        }
        if !open {
            return;
        }
        if let Some(step) = input.step {
            self.focus_shown = true;
            self.step(step, now);
        }
        if input.confirm
            && let Some(focus) = self.focus
        {
            self.focus_shown = true;
            self.activate(focus, now);
        }
        if input.scroll != Vec2::ZERO {
            self.scroll_by(input.scroll, now);
        }
        if input.back {
            self.events.push(UiEvent::Back);
            self.player.fire("back", now, &mut self.nodes);
        }
    }

    /// What's happened since this was last called. A screen nobody reads this of keeps it all, so one
    /// that takes [`Self::input`] drains it each frame.
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, UiEvent> {
        self.events.drain(..)
    }

    /// The node with focus.
    #[must_use]
    pub fn focus(&self) -> Option<UiNodeId> {
        self.focus.map(UiNodeId)
    }

    /// The name of the node with focus.
    #[must_use]
    pub fn focus_name(&self) -> Option<&str> {
        self.focus.map(|at| &*self.nodes[at as usize].name)
    }

    /// Moves focus to `node`, which a game does on opening a screen where the player left it. Whether it
    /// takes focus and is shown.
    pub fn set_focus(&mut self, node: UiNodeId, now: Timestamp) -> bool {
        let shown = &self.nodes[node.0 as usize];
        if shown.focus.is_none() || !shown.visible {
            return false;
        }
        self.move_focus(Some(node.0), now);
        true
    }

    /// Whether focus draws its look. A finger's press hides it, and a key or a pad's press shows it
    /// again, so a game starting on a phone turns it off.
    pub fn set_focus_shown(&mut self, shown: bool) {
        self.focus_shown = shown;
    }

    /// What's at `point`, by the last frame's picture: see [`UiHit`].
    #[must_use]
    pub fn hit(&self, point: Vec2) -> UiHit {
        for at in (0..self.nodes.len()).rev() {
            let node = &self.nodes[at];
            let shows =
                node.look.is_some() || node.text.is_some() || node.focus.is_some() || node.inset.is_some();
            if !shows || !node.visible || node.final_opacity <= 0.0 || !node.screen.contains(point) {
                continue;
            }
            if node
                .own_mask
                .is_some_and(|[x0, y0, x1, y1]| point.x < x0 || point.y < y0 || point.x > x1 || point.y > y1)
            {
                continue;
            }
            let mut walk = Some(at);
            while let Some(index) = walk {
                let over = &self.nodes[index];
                if over.focus.is_some() {
                    return UiHit::Focusable(UiNodeId(index as u32));
                }
                walk = over.parent.map(|parent| parent as usize);
            }
            return UiHit::Surface;
        }
        UiHit::Nothing
    }

    fn pointer(&mut self, pointer: UiPointer, open: bool) {
        let UiPointer { id, kind, phase, position, at } = pointer;
        if kind == UiPointerKind::Mouse {
            self.hover = Some(position);
        }
        let held = self.hold.is_some_and(|hold| hold.id == id && hold.kind == kind);
        match phase {
            UiPointerPhase::Down if open && self.hold.is_none() => self.press(pointer),
            UiPointerPhase::Move if held => self.drag(position, at),
            // Hovering is the mouse's: a finger is only anywhere while it's down.
            UiPointerPhase::Move if open && kind == UiPointerKind::Mouse && self.hold.is_none() => {
                if let UiHit::Focusable(node) = self.hit(position) {
                    self.focus_shown = true;
                    self.move_focus(Some(node.0), at);
                }
            }
            UiPointerPhase::Up if held => self.release(position, at, open),
            UiPointerPhase::Cancel if held => self.hold = None,
            _ => {}
        }
    }

    fn press(&mut self, pointer: UiPointer) {
        let UiPointer { id, kind, position, at, .. } = pointer;
        let node = match self.hit(position) {
            UiHit::Focusable(node) => Some(node.0),
            UiHit::Surface | UiHit::Nothing => None,
        };
        if let Some(node) = node {
            // A finger on a phone shows what it's pressing, and no highlight stays behind it.
            self.focus_shown = kind == UiPointerKind::Mouse;
            self.move_focus(Some(node), at);
        }
        // An inset under a finger takes its drag, to turn what stands in it, over a scroller it's in.
        let inset =
            self.inset_at(position).and_then(|inset| Some((inset, self.inset_yaw(inset)?, position.x)));
        let scroller = if inset.is_some() { None } else { self.scroller_at(position) };
        let start_offset = scroller.map_or(0.0, |scroller| {
            self.nodes[scroller as usize].scroll.as_ref().map_or(0.0, |scroll| scroll.offset(at))
        });
        if let Some(scroller) = scroller
            && let Some(scroll) = &mut self.nodes[scroller as usize].scroll
        {
            // A finger on a moving list stops it.
            scroll.hold_at(start_offset);
        }
        self.hold = Some(Hold {
            id,
            kind,
            start: position,
            last: position,
            last_at: at,
            velocity: 0.0,
            node,
            scroller,
            start_offset,
            inset,
            dragging: false,
        });
    }

    fn drag(&mut self, position: Vec2, at: Timestamp) {
        if let Some(hold) = &mut self.hold
            && let Some((inset, yaw, from)) = &mut hold.inset
        {
            if !hold.dragging && (position.x - *from).abs() >= DRAG_SLOP {
                // It turns from where the drag became one, so the slop doesn't jump it.
                hold.dragging = true;
                *from = position.x;
            }
            let (inset, yaw, moved) = (*inset, *yaw, position.x - *from);
            hold.last = position;
            hold.last_at = at;
            if hold.dragging {
                self.drag_inset(inset, yaw, moved);
            }
            return;
        }
        let Some(hold) = &mut self.hold else { return };
        let Some(scroller) = hold.scroller else { return };
        let Some(scroll) = &mut self.nodes[scroller as usize].scroll else { return };
        let moved = scroll.along(position - hold.start);
        if !hold.dragging && moved.abs() >= DRAG_SLOP {
            hold.dragging = true;
        }
        let seconds = at.saturating_duration_since(hold.last_at).as_secs_f32();
        if hold.dragging {
            scroll.hold_at(hold.start_offset - moved);
            if seconds > 0.0 {
                let velocity = -scroll.along(position - hold.last) / seconds;
                // The first sample has nothing to smooth with.
                let smoothing = if hold.velocity == 0.0 { 1.0 } else { FLICK_SMOOTHING };
                hold.velocity += (velocity - hold.velocity) * smoothing;
            }
        }
        hold.last = position;
        hold.last_at = at;
    }

    fn release(&mut self, position: Vec2, at: Timestamp, open: bool) {
        let Some(hold) = self.hold.take() else { return };
        if hold.dragging {
            if let Some(scroller) = hold.scroller
                && let Some(scroll) = &mut self.nodes[scroller as usize].scroll
            {
                let stopped = at.saturating_duration_since(hold.last_at).as_secs_f32() > FLICK_GONE;
                scroll.fling(if stopped { 0.0 } else { hold.velocity }, at);
            }
            return;
        }
        if open
            && let Some(node) = hold.node
            && self.hit(position) == UiHit::Focusable(UiNodeId(node))
        {
            self.activate(node, at);
        }
    }

    /// The node pressed right now, which draws its pressed look.
    pub(super) fn pressed(&self) -> Option<u32> {
        self.hold.filter(|hold| !hold.dragging).and_then(|hold| hold.node)
    }

    fn activate(&mut self, node: u32, now: Timestamp) {
        self.events.push(UiEvent::Activated(UiNodeId(node)));
        // Lent out for the call, which moves what the node belongs to.
        let rule = self.nodes[node as usize].focus.as_mut().and_then(|focus| focus.fires.take());
        if let Some(event) = rule {
            self.player.fire(&event, now, &mut self.nodes);
            if let Some(focus) = &mut self.nodes[node as usize].focus {
                focus.fires = Some(event);
            }
        }
    }

    fn step(&mut self, step: UiStep, now: Timestamp) {
        let Some(from) = self.focus else {
            self.settle_focus();
            return;
        };
        if let Some(to) = self.neighbour(from, step) {
            self.move_focus(Some(to), now);
        }
    }

    /// Where `step` goes from `from`: the node a document names, if it's shown, or the nearest one that
    /// way, counting a gap across twice so a straight line wins over a diagonal. The drawing order
    /// settles a tie, so a walk comes out the same every time.
    pub(super) fn neighbour(&self, from: u32, step: UiStep) -> Option<u32> {
        let rule = self.nodes[from as usize].focus.as_ref()?;
        let named = match step {
            UiStep::Up => rule.up,
            UiStep::Down => rule.down,
            UiStep::Left => rule.left,
            UiStep::Right => rule.right,
        };
        if let Some(named) = named
            && self.nodes[named as usize].visible
        {
            return Some(named);
        }
        let rect = self.nodes[from as usize].rect;
        let mut best = None;
        for &at in &self.focusable {
            let node = &self.nodes[at as usize];
            if at == from || !node.visible {
                continue;
            }
            let Some((gap, across, centers)) = step.distance(rect, node.rect) else { continue };
            let score = (gap + across * 2.0, centers);
            if best.is_none_or(|(known, _)| score < known) {
                best = Some((score, at));
            }
        }
        best.map(|(_, at)| at)
    }

    fn move_focus(&mut self, to: Option<u32>, now: Timestamp) {
        if self.focus == to {
            return;
        }
        self.events.push(UiEvent::FocusMoved { from: self.focus.map(UiNodeId), to: to.map(UiNodeId) });
        self.focus = to;
        if let Some(to) = to {
            self.reveal(to, now);
        }
    }

    /// Brings `node` into view in every scroller it's in.
    fn reveal(&mut self, node: u32, now: Timestamp) {
        let rect = self.nodes[node as usize].rect;
        let (mut near, mut far) = (rect.min, rect.max);
        let mut walk = self.nodes[node as usize].parent;
        while let Some(at) = walk {
            let over = &mut self.nodes[at as usize];
            walk = over.parent;
            let Some(scroll) = &mut over.scroll else { continue };
            let start = scroll.along(over.rect.min);
            let view = scroll.along(over.rect.max) - start;
            let (first, last) =
                (scroll.along(near) - start - REVEAL_ROOM, scroll.along(far) - start + REVEAL_ROOM);
            let target = scroll.target();
            let wanted = if first < target {
                first
            } else if last > target + view {
                last - view
            } else {
                target
            };
            if wanted != target {
                scroll.go_to(wanted, now);
            }
            // As an outer scroller would see it, from where this one is going.
            let moved = scroll.vector(scroll.target());
            near -= moved;
            far -= moved;
        }
    }

    /// The scroller a wheel or a finger at `point` moves: the innermost one under it.
    fn scroller_at(&self, point: Vec2) -> Option<u32> {
        (0..self.nodes.len()).rev().map(|at| at as u32).find(|&at| {
            let node = &self.nodes[at as usize];
            node.scroll.as_ref().is_some_and(|scroll| scroll.extent > 0.0)
                && node.visible
                && node.screen.contains(point)
                && node.own_mask.is_none_or(|[x0, y0, x1, y1]| {
                    point.x >= x0 && point.y >= y0 && point.x <= x1 && point.y <= y1
                })
        })
    }

    /// Goes through what the pointer is over, or else what holds focus, or else the first thing that
    /// scrolls.
    fn scroll_by(&mut self, amount: Vec2, now: Timestamp) {
        let around_focus = || {
            let mut walk = self.focus;
            while let Some(at) = walk {
                let node = &self.nodes[at as usize];
                if node.scroll.is_some() {
                    return Some(at);
                }
                walk = node.parent;
            }
            None
        };
        let scroller = self
            .hover
            .and_then(|point| self.scroller_at(point))
            .or_else(around_focus)
            .or_else(|| self.nodes.iter().position(|node| node.scroll.is_some()).map(|at| at as u32));
        let Some(scroller) = scroller else { return };
        let Some(scroll) = &mut self.nodes[scroller as usize].scroll else { return };
        let along = scroll.along(amount);
        // A wheel with one axis still moves a list on the other.
        let along = if along == 0.0 { amount.x + amount.y } else { along };
        scroll.go_to(scroll.target() + along, now);
    }

    /// Keeps focus on something shown: what held it may have just gone away.
    pub(super) fn settle_focus(&mut self) {
        let good = self.focus.is_some_and(|at| self.nodes[at as usize].visible);
        if good {
            return;
        }
        let from = self.focus;
        let visible = |at: &&u32| self.nodes[**at as usize].visible;
        let next =
            from.and_then(|from| self.focusable.iter().filter(visible).find(|at| **at > from))
                .or_else(|| {
                    self.first.filter(|at| self.nodes[*at as usize].visible).as_ref().map(|_| {
                        self.focusable.iter().find(|at| Some(**at) == self.first).expect("one of them")
                    })
                })
                .or_else(|| self.focusable.iter().find(visible))
                .copied();
        if next != from {
            self.events.push(UiEvent::FocusMoved { from: from.map(UiNodeId), to: next.map(UiNodeId) });
            self.focus = next;
        }
    }

    /// What a node draws behind its children right now.
    pub(super) fn look_of(&self, at: usize) -> Option<&UiLook> {
        let node = &self.nodes[at];
        let Some(focus) = &node.focus else { return node.look.as_ref() };
        let index = Some(at as u32);
        if self.pressed() == index
            && let Some(pressed) = &focus.pressed
        {
            return Some(pressed);
        }
        if self.focus_shown
            && self.focus == index
            && let Some(focused) = &focus.focused
        {
            return Some(focused);
        }
        node.look.as_ref()
    }

    /// Runs a walk of steps, however a test or a capture wrote them, and says where focus was after each.
    /// `confirm` activates; the rest are [`UiStep`]s by name. A walk runs whatever the machine's state.
    ///
    /// # Errors
    ///
    /// A step that's neither.
    pub fn walk<'a>(
        &mut self,
        steps: impl IntoIterator<Item = &'a str>,
        now: Timestamp,
    ) -> Result<Vec<String>, String> {
        let mut seen = Vec::new();
        for name in steps {
            if name == "confirm" {
                if let Some(focus) = self.focus {
                    self.activate(focus, now);
                }
            } else {
                let step = UiStep::from_name(name)
                    .ok_or_else(|| format!("`{name}` isn't a step: up, down, left, right or confirm"))?;
                self.focus_shown = true;
                self.step(step, now);
            }
            seen.push(self.focus_name().unwrap_or_default().to_owned());
        }
        Ok(seen)
    }

    /// The shown nodes that take focus and that directions alone can't reach from where focus is: a
    /// screen with any has a player on a pad stuck out of them. What a scripted walk's test holds at
    /// each aspect a screen ships at.
    #[must_use]
    pub fn unreachable(&self) -> Vec<&str> {
        let mut reached: Vec<u32> = self.focus.into_iter().collect();
        let mut next = 0;
        while let Some(&from) = reached.get(next) {
            next += 1;
            for step in [UiStep::Up, UiStep::Down, UiStep::Left, UiStep::Right] {
                if let Some(to) = self.neighbour(from, step)
                    && !reached.contains(&to)
                {
                    reached.push(to);
                }
            }
        }
        self.focusable
            .iter()
            .filter(|at| self.nodes[**at as usize].visible && !reached.contains(at))
            .map(|at| &*self.nodes[*at as usize].name)
            .collect()
    }
}
