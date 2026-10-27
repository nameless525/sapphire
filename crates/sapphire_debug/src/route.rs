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
//!   screens, holding two fingers still for a second. The fingers of a hold that toggles are
//!   taken from whoever had them: the game gets a cancel for each of its own
//!   (`TouchPhase::Canceled`, which fires nothing a release would), and their remaining events go
//!   nowhere.
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
/// How far, in window coordinates (points on phones), a holding finger may drift.
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
    /// Where it was when the hold began, in window coordinates.
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
    /// The window's size in window coordinates, which touch positions are normalized to.
    window_size: Size,
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
            window_size: Size { width: 1, height: 1 },
            hold_since: None,
            claimed: Vec::with_capacity(2),
        }
    }

    pub fn set_window_size(&mut self, size: Size) {
        self.window_size = size;
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
            if let Some(finger) = finger {
                self.claimed.push(Claimed { slot, owner: finger.owner, touch: finger.last });
                finger.owner = Owner::Nobody;
            }
        }
        true
    }

    fn touch(&mut self, touch: &TouchEvent, ui: &UiState<'_>) -> Route {
        let existing = self.fingers.iter().position(|f| f.is_some_and(|f| f.id == touch.finger));
        let position = (touch.x * self.window_size.width as f32, touch.y * self.window_size.height as f32);
        let slot = match (existing, touch.phase) {
            (Some(slot), _) => slot,
            (None, TouchPhase::Down) => {
                let Some(free) = self.fingers.iter().position(Option::is_none) else {
                    return Route::default();
                };
                let over = ui.visible && (ui.covers)(position.0, position.1);
                let owner = if over { Owner::Overlay } else { Owner::Game };
                self.fingers[free] = Some(Finger { id: touch.finger, owner, last: *touch, anchor: position });
                self.rearm_hold(touch.timestamp);
                free
            }
            // A finger landed while every slot was taken: it has no owner.
            (None, _) => return Route::default(),
        };
        let finger = self.fingers[slot].as_mut().expect("the slot is taken");
        finger.last = *touch;
        let owner = finger.owner;
        let (dx, dy) = (position.0 - finger.anchor.0, position.1 - finger.anchor.1);
        let drifted = dx * dx + dy * dy > HOLD_SLOP * HOLD_SLOP;
        if matches!(touch.phase, TouchPhase::Up | TouchPhase::Canceled) {
            self.fingers[slot] = None;
            self.rearm_hold(touch.timestamp);
        } else if drifted {
            self.hold_since = None;
        }
        Route { finger: Some(slot), ..Self::to(owner, ui) }
    }

    /// Starts timing a hold if exactly two fingers are down and neither was claimed yet, from
    /// where they are now.
    fn rearm_hold(&mut self, now: Timestamp) {
        let mut down = self.fingers.iter_mut().flatten();
        let pair = (down.next(), down.next(), down.next());
        self.hold_since = match pair {
            (Some(a), Some(b), None) if a.owner != Owner::Nobody && b.owner != Owner::Nobody => {
                for finger in [a, b] {
                    let last = finger.last;
                    finger.anchor =
                        (last.x * self.window_size.width as f32, last.y * self.window_size.height as f32);
                }
                Some(now)
            }
            _ => None,
        };
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
        KeyEvent, Keycode, Modifiers, MouseButtonEvent, MouseMotionEvent, MouseWheelEvent, TextEvent,
        TouchDeviceId, WindowId,
    };

    use super::*;

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
}
