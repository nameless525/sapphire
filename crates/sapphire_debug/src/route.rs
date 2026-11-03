//! Which input the overlay takes and which the game keeps: the overlay sees every platform event
//! first, and the game sees only what the overlay doesn't use.
//!
//! The rules, all decided per event from what the overlay drew last frame:
//!
//! - **Presses go to one side.** A mouse button or a finger that goes down over an overlay window
//!   belongs to the overlay until it is released; anywhere else, to the game. Its moves and its
//!   release follow it, so a camera drag that starts in the game keeps going over a window, and
//!   a slider drag never turns the camera.
//! - **Pointer motion** reaches the game unless the overlay owns a held button, or the pointer
//!   is over a window with no game-owned button held. The overlay sees all of it, for hover.
//!   The wheel goes to whichever side the pointer is over.
//! - **Keys** go to the overlay alone while one of its text fields has focus, and to the game
//!   otherwise. Releases always reach the game, so nothing it holds sticks.
//! - **Losing focus or the foreground** reaches both, and forgets every press.
//! - **The toggle** belongs to neither: the toggle key (`` ` `` by default) and, on touch
//!   screens, holding two fingers still for a second, which is timed only once a frame has given
//!   the window's size to measure their drift in. The fingers of a hold that toggles are
//!   taken from whoever had them: the game gets a cancel for each of its own
//!   (`TouchPhase::Canceled`, which fires nothing a release would), and their remaining events go
//!   nowhere. A finger on one of the game's on-screen controls isn't counted at all
//!   (`Router::claim_touch`): a thumb resting on a stick and another on a button are playing.
//! - Everything else (window, lifecycle, gamepad events) reaches the game, and window events the
//!   overlay too.
//!
//! Hidden, the overlay takes nothing but the toggle.

use std::time::Duration;

use sapphire_input::MAX_FINGERS;
use sapphire_platform::{
    Event, FingerId, LifecycleEvent, MouseButton, Scancode, Size, Timestamp, TouchEvent, TouchPhase,
    WindowEvent,
};

/// How long two fingers hold still to toggle the overlay.
pub const HOLD_TIME: Duration = Duration::from_secs(1);
/// How far, in points, a holding finger may drift.
const HOLD_SLOP: f32 = 16.0;

/// What the overlay's last frame says about it.
pub(crate) struct UiState<'a> {
    pub visible: bool,
    /// A text field has keyboard focus.
    pub wants_keyboard: bool,
    /// Whether an overlay window covers a point in window coordinates.
    pub covers: &'a dyn Fn(f32, f32) -> bool,
}

/// Where one event goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Route {
    pub game: bool,
    pub overlay: bool,
    /// The touch's finger slot, for the overlay's touch ids.
    pub finger: Option<usize>,
    /// The event toggled the overlay.
    pub toggle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Game,
    Overlay,
    /// Taken by the toggle gesture: nobody sees the rest of it.
    Nobody,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: FingerId,
    owner: Owner,
    /// Its latest event, to cancel it with.
    last: TouchEvent,
    /// The game's on-screen controls hold it, so the toggle doesn't count it.
    control: bool,
    /// Where it was when the hold began, in points.
    anchor: (f32, f32),
}

/// A finger the toggle gesture took, and who had it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Claimed {
    pub slot: usize,
    pub owner: Owner,
    pub touch: TouchEvent,
}

/// Routing state: who owns each held button and finger.
#[derive(Debug)]
pub(crate) struct Router {
    toggle_key: Scancode,
    /// Left, middle, right, X1, X2.
    buttons: [Option<Owner>; 5],
    fingers: [Option<Finger>; MAX_FINGERS],
    /// The mouse, in window coordinates.
    pointer: (f32, f32),
    /// The window's size in window coordinates, which touch positions are normalized to, once a
    /// frame has said it. Until then no hold is timed: without the size a finger's drift can't
    /// be measured, and a pinch would pass for two fingers holding still.
    window_size: Option<Size>,
    /// What a window coordinate is in points, which a finger's drift is measured in.
    points_per_coordinate: f32,
    /// When two still fingers started holding.
    hold_since: Option<Timestamp>,
    /// What the last toggle gesture took, for the overlay to cancel.
    claimed: Vec<Claimed>,
}

impl Router {
    pub fn new(toggle_key: Scancode) -> Self {
        Self {
            toggle_key,
            buttons: [None; 5],
            fingers: [None; MAX_FINGERS],
            pointer: (0.0, 0.0),
            window_size: None,
            points_per_coordinate: 1.0,
            hold_since: None,
            claimed: Vec::with_capacity(2),
        }
    }

    /// The window's size in window coordinates now. Call it every frame, shown or hidden, before
    /// the frame's events: the toggle gesture measures drift with it. An empty window has none.
    pub fn set_window_size(&mut self, size: Size) {
        self.window_size = Some(size).filter(|size| size.width > 0 && size.height > 0);
    }

    /// What a window coordinate is in points now. Call it with [`Self::set_window_size`].
    pub fn set_points_per_coordinate(&mut self, points: f32) {
        self.points_per_coordinate = points;
    }

    /// The fingers the last toggle took from their owners.
    pub fn take_claimed(&mut self) -> std::vec::Drain<'_, Claimed> {
        self.claimed.drain(..)
    }

    /// Decides where `event` goes.
    pub fn route(&mut self, event: &Event, ui: &UiState<'_>) -> Route {
        let both = Route { game: true, overlay: ui.visible, ..Route::default() };
        let game = Route { game: true, ..Route::default() };
        let overlay = Route { overlay: ui.visible, ..Route::default() };
        match *event {
            Event::Key(key) if key.scancode == self.toggle_key => {
                Route { toggle: key.down && !key.repeat, ..Route::default() }
            }
            Event::Key(key) if !key.down => both,
            Event::Key(_) | Event::Text(_) if ui.visible && ui.wants_keyboard => overlay,
            Event::Key(_) | Event::Text(_) => game,
            Event::MouseButton(button) => {
                self.pointer = (button.x, button.y);
                let Some(index) = button_index(button.button) else { return game };
                if button.down {
                    let owner = if ui.visible && (ui.covers)(button.x, button.y) {
                        Owner::Overlay
                    } else {
                        Owner::Game
                    };
                    self.buttons[index] = Some(owner);
                    Self::to(owner, ui)
                } else {
                    match self.buttons[index].take() {
                        Some(owner) => Self::to(owner, ui),
                        None => both,
                    }
                }
            }
            Event::MouseMotion(motion) => {
                self.pointer = (motion.x, motion.y);
                let held = |owner| self.buttons.contains(&Some(owner));
                let over = ui.visible && (ui.covers)(motion.x, motion.y);
                let to_game = !held(Owner::Overlay) && !(over && !held(Owner::Game));
                Route { game: to_game, overlay: ui.visible, ..Route::default() }
            }
            Event::MouseWheel(_) => {
                if ui.visible && (ui.covers)(self.pointer.0, self.pointer.1) {
                    overlay
                } else {
                    game
                }
            }
            Event::Touch(touch) => self.touch(&touch, ui),
            Event::Window { event: WindowEvent::FocusLost, .. }
            | Event::Lifecycle { event: LifecycleEvent::WillEnterBackground, .. } => {
                self.forget();
                both
            }
            Event::Window { .. } => both,
            _ => game,
        }
    }

    /// Checks the two-finger hold at `now`; `true` if it completed (see [`Self::take_claimed`]).
    pub fn poll(&mut self, now: Timestamp) -> bool {
        let Some(since) = self.hold_since else { return false };
        if now.saturating_duration_since(since) < HOLD_TIME {
            return false;
        }
        self.hold_since = None;
        for (slot, finger) in self.fingers.iter_mut().enumerate() {
            if let Some(finger) = finger.as_mut().filter(|finger| !finger.control) {
                self.claimed.push(Claimed { slot, owner: finger.owner, touch: finger.last });
                finger.owner = Owner::Nobody;
            }
        }
        true
    }

    fn touch(&mut self, touch: &TouchEvent, ui: &UiState<'_>) -> Route {
        let existing = self.fingers.iter().position(|f| f.is_some_and(|f| f.id == touch.finger));
        let position = self.position(touch);
        let slot = match (existing, touch.phase) {
            (Some(slot), _) => slot,
            (None, TouchPhase::Down) => {
                let Some(free) = self.fingers.iter().position(Option::is_none) else {
                    return Route::default();
                };
                // Before the first frame no overlay window has been drawn to cover the finger.
                let over = ui.visible
                    && self.window_size.is_some_and(|size| {
                        let (x, y) = in_window(touch, size);
                        (ui.covers)(x, y)
                    });
                let owner = if over { Owner::Overlay } else { Owner::Game };
                let anchor = position.unwrap_or_default();
                self.fingers[free] =
                    Some(Finger { id: touch.finger, owner, control: false, last: *touch, anchor });
                self.rearm_hold(touch.timestamp);
                free
            }
            // A finger landed while every slot was taken: it has no owner.
            (None, _) => return Route::default(),
        };
        let finger = self.fingers[slot].as_mut().expect("the slot is taken");
        finger.last = *touch;
        let (owner, control) = (finger.owner, finger.control);
        // Without the window's size no hold is being timed, so there is no drift to measure.
        let drifted = position.is_some_and(|(x, y)| {
            let (dx, dy) = (x - finger.anchor.0, y - finger.anchor.1);
            dx * dx + dy * dy > HOLD_SLOP * HOLD_SLOP
        });
        if matches!(touch.phase, TouchPhase::Up | TouchPhase::Canceled) {
            self.fingers[slot] = None;
            if !control {
                self.rearm_hold(touch.timestamp);
            }
        } else if drifted && !control {
            self.hold_since = None;
        }
        Route { finger: Some(slot), ..Self::to(owner, ui) }
    }

    /// The game's on-screen controls hold this finger until it lifts: the toggle stops counting it,
    /// and a hold doesn't take it. Other fingers still can toggle, on whatever isn't a control.
    pub fn claim_touch(&mut self, finger: FingerId, now: Timestamp) {
        let held = self.fingers.iter_mut().flatten().find(|held| held.id == finger);
        if let Some(held) = held.filter(|held| !held.control) {
            held.control = true;
            self.rearm_hold(now);
        }
    }

    /// Starts timing a hold if exactly two fingers no control holds are down and neither was claimed
    /// yet, from where they are now, once the window's size is known.
    fn rearm_hold(&mut self, now: Timestamp) {
        let Some(size) = self.window_size else {
            self.hold_since = None;
            return;
        };
        let points = self.points_per_coordinate;
        let mut down = self.fingers.iter_mut().flatten().filter(|finger| !finger.control);
        let pair = (down.next(), down.next(), down.next());
        self.hold_since = match pair {
            (Some(a), Some(b), None) if a.owner != Owner::Nobody && b.owner != Owner::Nobody => {
                for finger in [a, b] {
                    finger.anchor = in_points(&finger.last, size, points);
                }
                Some(now)
            }
            _ => None,
        };
    }

    /// Where `touch` is in points, once the window's size is known.
    fn position(&self, touch: &TouchEvent) -> Option<(f32, f32)> {
        self.window_size.map(|size| in_points(touch, size, self.points_per_coordinate))
    }

    fn forget(&mut self) {
        self.buttons = [None; 5];
        self.fingers = [None; MAX_FINGERS];
        self.hold_since = None;
    }

    fn to(owner: Owner, ui: &UiState<'_>) -> Route {
        match owner {
            Owner::Game => Route { game: true, ..Route::default() },
            Owner::Overlay => Route { overlay: ui.visible, ..Route::default() },
            Owner::Nobody => Route::default(),
        }
    }
}

/// A touch's normalized position, in the window coordinates of a window `size` of them: what
/// [`UiState::covers`] takes, as a mouse's positions are.
fn in_window(touch: &TouchEvent, size: Size) -> (f32, f32) {
    (touch.x * size.width as f32, touch.y * size.height as f32)
}

/// A touch's normalized position, in points, on a window of `size` window coordinates: what a hold's
/// drift is measured in.
fn in_points(touch: &TouchEvent, size: Size, points_per_coordinate: f32) -> (f32, f32) {
    let (x, y) = in_window(touch, size);
    (x * points_per_coordinate, y * points_per_coordinate)
}

fn button_index(button: MouseButton) -> Option<usize> {
    Some(match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
        MouseButton::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use sapphire_platform::{
        KeyEvent, Keycode, Modifiers, MouseButtonEvent, MouseMotionEvent, MouseWheelEvent, Rect, TextEvent,
        TouchDeviceId, WindowId,
    };

    use super::*;
    use crate::input::{EguiInput, WindowMetrics};

    const WINDOW: WindowId = WindowId::from_raw(1);

    fn ms(ms: u64) -> Timestamp {
        Timestamp::from_nanos(ms * 1_000_000)
    }

    /// An overlay window over the left half of a 1000 × 500 window.
    fn covers(x: f32, _y: f32) -> bool {
        x < 500.0
    }

    fn ui(visible: bool, wants_keyboard: bool) -> UiState<'static> {
        UiState { visible, wants_keyboard, covers: &covers }
    }

    fn router() -> Router {
        let mut router = Router::new(Scancode::GRAVE);
        router.set_window_size(Size { width: 1000, height: 500 });
        router
    }

    fn key(scancode: Scancode, down: bool, repeat: bool) -> Event {
        Event::Key(KeyEvent {
            timestamp: ms(1),
            window: WINDOW,
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat,
        })
    }

    fn button(down: bool, x: f32) -> Event {
        Event::MouseButton(MouseButtonEvent {
            timestamp: ms(1),
            window: WINDOW,
            button: MouseButton::Left,
            down,
            clicks: 1,
            x,
            y: 100.0,
        })
    }

    fn motion(x: f32) -> Event {
        Event::MouseMotion(MouseMotionEvent {
            timestamp: ms(1),
            window: WINDOW,
            x,
            y: 100.0,
            dx: 1.0,
            dy: 0.0,
        })
    }

    fn touch(t: u64, finger: u64, phase: TouchPhase, x: f32) -> Event {
        Event::Touch(TouchEvent {
            timestamp: ms(t),
            window: WINDOW,
            finger: FingerId::new(TouchDeviceId::from_raw(1), finger),
            phase,
            x,
            y: 0.5,
            pressure: 1.0,
        })
    }

    fn to(route: Route) -> (bool, bool) {
        (route.game, route.overlay)
    }

    #[test]
    fn the_toggle_key_belongs_to_neither_side() {
        let mut router = router();
        let toggled = router.route(&key(Scancode::GRAVE, true, false), &ui(false, false));
        assert_eq!(toggled, Route { toggle: true, ..Route::default() });
        assert!(!router.route(&key(Scancode::GRAVE, true, true), &ui(true, true)).toggle, "repeats don't");
        assert_eq!(router.route(&key(Scancode::GRAVE, false, false), &ui(true, false)), Route::default());
    }

    #[test]
    fn keys_go_to_a_focused_text_field_and_releases_always_to_the_game() {
        let mut router = router();
        assert_eq!(to(router.route(&key(Scancode::W, true, false), &ui(true, false))), (true, false));
        assert_eq!(to(router.route(&key(Scancode::W, true, false), &ui(true, true))), (false, true));
        assert_eq!(to(router.route(&key(Scancode::W, false, false), &ui(true, true))), (true, true));
        assert_eq!(
            to(router.route(&key(Scancode::W, true, false), &ui(false, true))),
            (true, false),
            "hidden"
        );
        let text = Event::Text(TextEvent::split(ms(1), WINDOW, "w").next().expect("text"));
        assert_eq!(to(router.route(&text, &ui(true, true))), (false, true));
        assert_eq!(to(router.route(&text, &ui(true, false))), (true, false));
    }

    #[test]
    fn mouse_presses_belong_to_where_they_started() {
        let mut router = router();
        let visible = ui(true, false);
        // A drag that starts in the game stays the game's over a window.
        assert_eq!(to(router.route(&button(true, 800.0), &visible)), (true, false));
        assert_eq!(to(router.route(&motion(200.0), &visible)), (true, true));
        assert_eq!(to(router.route(&button(false, 200.0), &visible)), (true, false));
        // Hovering a window hides motion from the game; a slider drag keeps it hidden off it.
        assert_eq!(to(router.route(&motion(200.0), &visible)), (false, true));
        assert_eq!(to(router.route(&button(true, 200.0), &visible)), (false, true));
        assert_eq!(to(router.route(&motion(800.0), &visible)), (false, true));
        assert_eq!(to(router.route(&button(false, 800.0), &visible)), (false, true));
        assert_eq!(to(router.route(&motion(800.0), &visible)), (true, true));
        // The wheel goes where the pointer is.
        let wheel = Event::MouseWheel(MouseWheelEvent { timestamp: ms(1), window: WINDOW, x: 0.0, y: 1.0 });
        assert_eq!(to(router.route(&wheel, &visible)), (true, false));
        router.route(&motion(100.0), &visible);
        assert_eq!(to(router.route(&wheel, &visible)), (false, true));
        // Hidden, everything is the game's.
        assert_eq!(to(router.route(&button(true, 100.0), &ui(false, false))), (true, false));
        assert_eq!(to(router.route(&wheel, &ui(false, false))), (true, false));
    }

    #[test]
    fn losing_focus_reaches_both_and_forgets_presses() {
        let mut router = router();
        router.route(&button(true, 100.0), &ui(true, false));
        let lost = Event::Window { timestamp: ms(2), window: WINDOW, event: WindowEvent::FocusLost };
        assert_eq!(to(router.route(&lost, &ui(true, false))), (true, true));
        assert_eq!(
            to(router.route(&button(false, 100.0), &ui(true, false))),
            (true, true),
            "unknown release"
        );
        let background = Event::Lifecycle { timestamp: ms(3), event: LifecycleEvent::WillEnterBackground };
        assert!(router.route(&background, &ui(false, false)).game);
        let quit = Event::Quit { timestamp: ms(4) };
        assert_eq!(to(router.route(&quit, &ui(true, false))), (true, false));
    }

    #[test]
    fn fingers_follow_their_owner() {
        let mut router = router();
        let visible = ui(true, false);
        let game = router.route(&touch(0, 1, TouchPhase::Down, 0.9), &visible);
        assert_eq!((to(game), game.finger), ((true, false), Some(0)));
        let overlay = router.route(&touch(1, 2, TouchPhase::Down, 0.1), &visible);
        assert_eq!((to(overlay), overlay.finger), ((false, true), Some(1)));
        assert_eq!(to(router.route(&touch(2, 1, TouchPhase::Moved, 0.2), &visible)), (true, false));
        assert_eq!(to(router.route(&touch(3, 2, TouchPhase::Up, 0.8), &visible)), (false, true));
        assert_eq!(
            router.route(&touch(4, 3, TouchPhase::Down, 0.1), &visible).finger,
            Some(1),
            "slots are reused"
        );
        assert_eq!(router.route(&touch(5, 9, TouchPhase::Moved, 0.1), &visible), Route::default(), "unknown");
    }

    #[test]
    fn a_two_finger_hold_toggles_and_takes_the_fingers() {
        let mut router = router();
        let hidden = ui(false, false);
        router.route(&touch(0, 1, TouchPhase::Down, 0.6), &hidden);
        assert!(!router.poll(ms(2000)), "one finger never toggles");
        router.route(&touch(100, 2, TouchPhase::Down, 0.8), &hidden);
        // A little jitter is still holding.
        router.route(&touch(500, 1, TouchPhase::Moved, 0.605), &hidden);
        assert!(!router.poll(ms(1099)));
        assert!(router.poll(ms(1100)), "a second after the second finger landed");
        let claimed: Vec<Claimed> = router.take_claimed().collect();
        assert_eq!(claimed.len(), 2);
        assert!(claimed.iter().all(|claimed| claimed.owner == Owner::Game));
        assert_eq!(claimed[0].touch.x, 0.605, "cancelled where it last was");
        assert!(!router.poll(ms(5000)), "once per hold");
        assert_eq!(
            router.route(&touch(5100, 1, TouchPhase::Moved, 0.7), &hidden),
            Route { finger: Some(0), ..Route::default() }
        );
        assert_eq!(to(router.route(&touch(5200, 1, TouchPhase::Up, 0.7), &hidden)), (false, false));
        router.route(&touch(5300, 2, TouchPhase::Up, 0.8), &hidden);
        assert_eq!(
            to(router.route(&touch(5400, 3, TouchPhase::Down, 0.5), &hidden)),
            (true, false),
            "fresh again"
        );
    }

    #[test]
    fn moving_or_a_third_finger_cancels_the_hold() {
        let mut router = router();
        let hidden = ui(false, false);
        router.route(&touch(0, 1, TouchPhase::Down, 0.2), &hidden);
        router.route(&touch(0, 2, TouchPhase::Down, 0.4), &hidden);
        // 0.05 of 1000 units is a pinch, not a hold.
        router.route(&touch(300, 2, TouchPhase::Moved, 0.45), &hidden);
        assert!(!router.poll(ms(2000)));

        let mut router = Router::new(Scancode::GRAVE);
        router.set_window_size(Size { width: 1000, height: 500 });
        for finger in 1..=3 {
            router.route(&touch(0, finger, TouchPhase::Down, 0.1 * finger as f32), &hidden);
        }
        assert!(!router.poll(ms(2000)));
        // Lifting the third leaves a fresh pair, which may hold.
        router.route(&touch(2000, 3, TouchPhase::Up, 0.3), &hidden);
        assert!(router.poll(ms(3000)));
    }

    fn finger_id(finger: u64) -> FingerId {
        FingerId::new(TouchDeviceId::from_raw(1), finger)
    }

    /// A thumb on a stick and another on a button, held still and for as long as you like, are playing:
    /// the game's controls claim them, and the toggle never counts them. Two other fingers still toggle,
    /// and take only themselves.
    #[test]
    fn fingers_on_the_games_controls_never_toggle() {
        let fresh = router;
        let mut router = fresh();
        let hidden = ui(false, false);
        let (stick, button, camera, spare) = (1, 2, 3, 4);
        router.route(&touch(1, stick, TouchPhase::Down, 0.1), &hidden);
        router.claim_touch(finger_id(stick), ms(1));
        router.route(&touch(2, button, TouchPhase::Down, 0.9), &hidden);
        router.claim_touch(finger_id(button), ms(2));
        assert!(!router.poll(ms(5000)), "playing");

        // A third and a fourth finger make a pair, and a thumb working its stick doesn't disturb them.
        router.route(&touch(5000, camera, TouchPhase::Down, 0.5), &hidden);
        router.route(&touch(5001, spare, TouchPhase::Down, 0.6), &hidden);
        let moved = router.route(&touch(5500, stick, TouchPhase::Moved, 0.2), &hidden);
        assert!(moved.game, "the stick's still the game's");
        assert!(!router.poll(ms(5900)), "not yet");
        assert!(router.poll(ms(6100)), "held for a second");
        let taken: Vec<_> = router.take_claimed().map(|claimed| claimed.touch.finger).collect();
        assert_eq!(taken, [finger_id(camera), finger_id(spare)], "and the controls keep theirs");
        assert!(router.route(&touch(6200, stick, TouchPhase::Moved, 0.25), &hidden).game);
        assert!(!router.route(&touch(6200, camera, TouchPhase::Moved, 0.5), &hidden).game);

        // A claim that comes after the pair formed stops its hold, and one for nobody does nothing.
        let mut router = fresh();
        router.route(&touch(1, stick, TouchPhase::Down, 0.1), &hidden);
        router.route(&touch(2, camera, TouchPhase::Down, 0.5), &hidden);
        router.claim_touch(finger_id(stick), ms(3));
        router.claim_touch(finger_id(9), ms(3));
        assert!(!router.poll(ms(5000)));

        // Its lift leaves the rest as they were.
        let mut router = fresh();
        router.route(&touch(1, stick, TouchPhase::Down, 0.1), &hidden);
        router.claim_touch(finger_id(stick), ms(1));
        router.route(&touch(10, camera, TouchPhase::Down, 0.5), &hidden);
        router.route(&touch(20, spare, TouchPhase::Down, 0.6), &hidden);
        router.route(&touch(900, stick, TouchPhase::Up, 0.1), &hidden);
        assert!(router.poll(ms(1100)), "the hold began at 20 ms");
    }

    /// A pinch: two fingers spreading for 1.1 s, each moving 0.33 of the window's width.
    fn pinch(router: &mut Router, ui: &UiState<'_>) {
        router.route(&touch(0, 1, TouchPhase::Down, 0.4), ui);
        router.route(&touch(0, 2, TouchPhase::Down, 0.6), ui);
        for step in 1..=11 {
            let spread = 0.03 * step as f32;
            router.route(&touch(100 * step, 1, TouchPhase::Moved, 0.4 - spread), ui);
            router.route(&touch(100 * step, 2, TouchPhase::Moved, 0.6 + spread), ui);
        }
    }

    /// Two fingers holding still for 1.1 s.
    fn hold(router: &mut Router, ui: &UiState<'_>) {
        router.route(&touch(0, 1, TouchPhase::Down, 0.4), ui);
        router.route(&touch(0, 2, TouchPhase::Down, 0.6), ui);
        router.route(&touch(500, 1, TouchPhase::Moved, 0.402), ui);
    }

    fn lift(router: &mut Router, ui: &UiState<'_>) {
        router.route(&touch(6000, 1, TouchPhase::Up, 0.5), ui);
        router.route(&touch(6000, 2, TouchPhase::Up, 0.5), ui);
    }

    /// Two fingers drifting `by` coordinates across a window 2400 wide, for over a second.
    fn drifting(router: &mut Router, ui: &UiState<'_>, by: f32) -> bool {
        router.route(&touch(0, 1, TouchPhase::Down, 0.4), ui);
        router.route(&touch(0, 2, TouchPhase::Down, 0.6), ui);
        router.route(&touch(500, 1, TouchPhase::Moved, 0.4 + by / 2400.0), ui);
        let toggled = router.poll(ms(1100));
        lift(router, ui);
        toggled
    }

    /// The drift a hold allows is the same size under a finger on a phone whose coordinates are pixels.
    #[test]
    fn a_holds_drift_is_measured_in_points() {
        let hidden = ui(false, false);
        let mut router = Router::new(Scancode::GRAVE);
        router.set_window_size(Size { width: 2400, height: 1080 });
        router.set_points_per_coordinate(1.0 / 3.0);
        assert!(drifting(&mut router, &hidden, 30.0), "10 points on a 3x phone is holding still");
        assert!(!drifting(&mut router, &hidden, 60.0), "20 points isn't");
        router.set_points_per_coordinate(1.0);
        assert!(!drifting(&mut router, &hidden, 30.0), "30 points isn't, where a coordinate is one");
    }

    /// A finger lands on the side a mouse press at that place would, whatever a window coordinate is
    /// in points and however far the overlay is zoomed: what's measured in points is its drift.
    #[test]
    fn a_finger_lands_where_a_press_there_would() {
        // Pixels and window coordinates across, and the display's scale: a coordinate is a point on
        // a Retina Mac, half of one on a 2× phone and a third on a 3× one.
        for (pixels, coordinates, scale) in [(2400_u32, 1200_u32, 2.0), (2400, 2400, 2.0), (2400, 2400, 3.0)]
        {
            for zoom in [0.5, 1.0, 1.5, 3.0] {
                let metrics = WindowMetrics {
                    pixel_size: Size { width: pixels, height: pixels / 2 },
                    size: Size { width: coordinates, height: coordinates / 2 },
                    display_scale: scale,
                    safe_area: Rect { x: 0, y: 0, width: coordinates, height: coordinates / 2 },
                    focused: true,
                };
                let mut input = EguiInput::default();
                input.set_metrics(&metrics, scale * zoom);
                // A panel across the left quarter of the screen, in egui's points, tested as
                // `DebugOverlay::handle_event` tests its windows.
                let panel = pixels as f32 / (scale * zoom) / 4.0;
                let covers = |x: f32, y: f32| input.to_points(x, y).x < panel;
                let visible = UiState { visible: true, wants_keyboard: false, covers: &covers };
                let mut router = Router::new(Scancode::GRAVE);
                router.set_window_size(metrics.size);
                router.set_points_per_coordinate(metrics.points_per_coordinate());
                for (across, on_panel) in
                    [(0.1, true), (0.24, true), (0.26, false), (0.6, false), (0.9, false)]
                {
                    let owner = (!on_panel, on_panel);
                    let at = across * coordinates as f32;
                    let what = format!("{across} across, {pixels}/{coordinates}/{scale} at zoom {zoom}");
                    assert_eq!(to(router.route(&button(true, at), &visible)), owner, "a press {what}");
                    router.route(&button(false, at), &visible);
                    let down = router.route(&touch(1, 1, TouchPhase::Down, across), &visible);
                    assert_eq!(to(down), owner, "a finger {what}");
                    // It stays with its owner, off the panel or onto it, until it's cancelled.
                    let moved = router.route(&touch(2, 1, TouchPhase::Moved, 1.0 - across), &visible);
                    assert_eq!(to(moved), owner, "its move {what}");
                    let cancel = router.route(&touch(3, 1, TouchPhase::Canceled, 1.0 - across), &visible);
                    assert_eq!(to(cancel), owner, "its cancel {what}");
                }
            }
        }
    }

    /// The router hands the overlay window coordinates, exact at a panel's edge at any scale.
    #[test]
    fn a_fingers_position_reaches_the_overlay_in_window_coordinates() {
        let covers = |x: f32, y: f32| x < 600.0 && y < 270.0;
        let visible = UiState { visible: true, wants_keyboard: false, covers: &covers };
        for points_per_coordinate in [1.0, 0.5, 1.0 / 3.0] {
            let mut router = Router::new(Scancode::GRAVE);
            router.set_window_size(Size { width: 2400, height: 1080 });
            router.set_points_per_coordinate(points_per_coordinate);
            let landing = |router: &mut Router, x: f32, y: f32| {
                let down = Event::Touch(TouchEvent {
                    timestamp: ms(0),
                    window: WINDOW,
                    finger: finger_id(1),
                    phase: TouchPhase::Down,
                    x,
                    y,
                    pressure: 1.0,
                });
                let route = router.route(&down, &visible);
                router.forget();
                to(route)
            };
            assert_eq!(landing(&mut router, 0.2499, 0.2499), (false, true), "{points_per_coordinate}");
            assert_eq!(landing(&mut router, 0.25, 0.2499), (true, false), "{points_per_coordinate}");
            assert_eq!(landing(&mut router, 0.2499, 0.25), (true, false), "{points_per_coordinate}");
        }
    }

    #[test]
    fn no_hold_is_timed_until_the_window_size_is_known() {
        // DT-002: a router no frame has told the window's size yet sees no drift, so it times no
        // hold at all, still or not, and a pinch never toggles.
        let hidden = ui(false, false);
        let mut router = Router::new(Scancode::GRAVE);
        pinch(&mut router, &hidden);
        assert!(!router.poll(ms(1100)), "a moving pinch toggled");
        assert!(!router.poll(ms(5000)));
        lift(&mut router, &hidden);
        hold(&mut router, &hidden);
        assert!(!router.poll(ms(1100)), "no size, no hold");
        lift(&mut router, &hidden);
        router.set_window_size(Size { width: 0, height: 0 });
        hold(&mut router, &hidden);
        assert!(!router.poll(ms(1100)), "an empty window has no size");
        lift(&mut router, &hidden);

        // Once it is known, a pinch moves too far to hold and a still pair toggles, whichever
        // way the window turns.
        for size in [Size { width: 1000, height: 500 }, Size { width: 500, height: 1000 }] {
            router.set_window_size(size);
            pinch(&mut router, &hidden);
            assert!(!router.poll(ms(1100)), "{size:?}: a moving pinch toggled");
            lift(&mut router, &hidden);
            hold(&mut router, &hidden);
            assert!(router.poll(ms(1100)), "{size:?}: a still pair holds");
            lift(&mut router, &hidden);
        }
    }
}
