//! Fighters' faces on a fight's stage (M5 C5): what each fighter's face layer (`sapphire_anim::FaceLayer`) is
//! asked to show each frame, and the real seconds it moves by.
//!
//! A timeline's face key for the fighter's role comes first; past it, the face a conversation's line set for
//! that role, held until a later line sets it again or the conversation ends; past both, the fighter's own
//! resting face, which it blends back to. The faces run on real time, held by a pause and not by a cutscene
//! or a conversation that holds the simulation, so a face changes while the fight stands still. None of this
//! reaches the simulation: a fighter's morphs are drawn with its face over them, and its pose's own stay the
//! simulation's.
//!
//! A voiced line moves one face's mouth (M5 C6): the role its `mouth` names gets the voice's cooked mouth
//! curve, read where the audio system says the voice has got to. A pause holds the voice and so the mouth; a
//! line cut by the next, or by the conversation's end, shuts it, as does a voice that has run out.

use sapphire_anim::{FaceLayer, FaceOrder, Mouth};
use sapphire_core::Name;
use sapphire_dialogue::DialogueView;
use sapphire_sequencer::FaceCue;
use sapphire_stage::audio::Said;
use sapphire_timeline::DEFAULT_FACE_BLEND;

/// Set in a timeline's orders' keys, and in a conversation's, so that no key of one is ever the other's.
const FROM_TIMELINE: u64 = 1 << 63;
const FROM_DIALOGUE: u64 = 1 << 62;

/// How many roles' faces a conversation holds at once: past that, a new role takes the last's place.
const HELD: usize = 4;

/// What a timeline's face key asks, for the `generation`th timeline the stage has shown: a timeline played
/// again starts its faces again.
pub(crate) fn timeline_order(generation: u32, cue: FaceCue) -> FaceOrder {
    FaceOrder {
        key: FROM_TIMELINE | (u64::from(generation) << 32) | u64::from(cue.key),
        expression: cue.expression,
        weight: cue.weight,
        blend: cue.blend,
        since: cue.since,
    }
}

/// Moves `face` on by `dt` real seconds and asks it for `order`, or back to rest with none.
pub(crate) fn drive(face: &mut FaceLayer, dt: f32, order: Option<FaceOrder>) {
    face.advance(dt);
    match order {
        Some(order) => face.show(order),
        None => face.hand_back(DEFAULT_FACE_BLEND),
    }
}

/// The faces the open conversation's lines have set, by role, and whose mouth the line shown moves.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DialogueFaces {
    held: [Option<(Name, FaceOrder)>; HELD],
    /// The line they were last followed at, by the runner's count of lines shown.
    line: Option<u64>,
    /// The role the line shown names for its voice's mouth (M5 C6), and that line, by the same count.
    mouth: Option<(Name, u64)>,
}

impl DialogueFaces {
    /// Follows the conversation: a new line's faces take their roles' places, and with none open every face is
    /// let go. Allocates nothing.
    pub(crate) fn follow(&mut self, view: Option<&DialogueView<'_>>, lines_shown: u64) {
        let Some(view) = view else {
            *self = Self::default();
            return;
        };
        if self.line == Some(lines_shown) {
            return;
        }
        self.line = Some(lines_shown);
        self.mouth = view.mouth.map(|role| (Name::from_u64(role), lines_shown));
        for (index, face) in view.faces.iter().enumerate() {
            let role = Name::from_u64(face.role);
            let order = FaceOrder {
                key: FROM_DIALOGUE | ((lines_shown << 3 | index as u64) & !(FROM_TIMELINE | FROM_DIALOGUE)),
                expression: Name::from_u64(face.expression),
                weight: face.weight,
                blend: face.blend,
                since: 0.0,
            };
            let slot = self
                .held
                .iter()
                .position(|held| held.is_some_and(|(held, _)| held == role))
                .or_else(|| self.held.iter().position(Option::is_none))
                .unwrap_or(HELD - 1);
            self.held[slot] = Some((role, order));
        }
    }

    /// The faces held, with their roles.
    pub(crate) fn held(&self) -> impl Iterator<Item = (Name, FaceOrder)> + '_ {
        self.held.iter().flatten().copied()
    }

    /// The role whose mouth moves, and how (M5 C6): the line shown's role, with `said`'s mouth if the voice
    /// speaking is that line's, and shut if it isn't, or there's none, as before it starts, after it ends, or
    /// with no audio device. Every other face's mouth is shut.
    pub(crate) fn mouth(&self, said: Option<Said>) -> Option<(Name, Mouth)> {
        let (role, line) = self.mouth?;
        let mouth =
            said.filter(|said| said.serial == line).map_or(Mouth::CLOSED, |said| Mouth::said(said.mouth));
        Some((role, mouth))
    }
}

#[cfg(test)]
mod tests {
    use sapphire_dialogue::CookedFace;

    use super::*;

    fn view(faces: &[CookedFace]) -> DialogueView<'_> {
        DialogueView { speaker: "", line: "", vo: None, faces, mouth: None, choices: [None; 4], next: true }
    }

    fn face(role: &str, expression: &str) -> CookedFace {
        CookedFace {
            role: Name::new(role).as_u64(),
            expression: Name::new(expression).as_u64(),
            weight: 1.0,
            blend: 0.2,
        }
    }

    #[test]
    fn a_lines_faces_hold_until_another_line_sets_that_role_or_it_ends() {
        let mut faces = DialogueFaces::default();
        let first = [face("hero", "happy"), face("foe", "angry")];
        faces.follow(Some(&view(&first)), 1);
        let held: Vec<_> = faces.held().collect();
        assert_eq!(held.len(), 2);
        let hero_key = held[0].1.key;
        // A line with no faces keeps them, and the same line again is nothing new.
        faces.follow(Some(&view(&[])), 2);
        faces.follow(Some(&view(&first)), 2);
        assert_eq!(faces.held().count(), 2);
        assert_eq!(faces.held().next().unwrap().1.key, hero_key);
        // A later line sets the hero's again, with a new key: a new blend.
        faces.follow(Some(&view(&[face("hero", "sad")])), 3);
        let hero = faces.held().find(|(role, _)| *role == Name::new("hero")).unwrap().1;
        assert_eq!(hero.expression, Name::new("sad"));
        assert_ne!(hero.key, hero_key);
        assert_eq!(faces.held().count(), 2);
        faces.follow(None, 3);
        assert_eq!(faces.held().count(), 0);
    }

    /// M5 C6: the mouth is the line shown's, and only its own voice's opens it.
    #[test]
    fn a_lines_voice_moves_only_its_roles_mouth_and_only_its_own() {
        use sapphire_assets::mouth::MouthSample;
        let open = MouthSample { open: 1.0, visemes: [0.8, 0.0, 0.0, 0.0, 0.2] };
        let said = |serial| Some(Said { serial, mouth: open });
        let mut faces = DialogueFaces::default();
        let voiced = |mouth| DialogueView { mouth, ..view(&[]) };
        faces.follow(Some(&voiced(Some(Name::new("foe").as_u64()))), 1);
        assert_eq!(faces.mouth(said(1)), Some((Name::new("foe"), Mouth::said(open))));
        // The line before's voice, a voice that's ended, or none: shut.
        for heard in [said(0), None] {
            assert_eq!(faces.mouth(heard), Some((Name::new("foe"), Mouth::CLOSED)));
        }
        // A line that names no mouth, or a conversation's end, moves none.
        faces.follow(Some(&voiced(None)), 2);
        assert_eq!(faces.mouth(said(2)), None);
        faces.follow(Some(&voiced(Some(Name::new("foe").as_u64()))), 3);
        faces.follow(None, 3);
        assert_eq!(faces.mouth(said(3)), None);
    }

    #[test]
    fn a_timelines_keys_are_apart_from_a_conversations_and_from_another_runs() {
        let cue = FaceCue { expression: Name::new("happy"), weight: 1.0, blend: 0.2, key: 1, since: 0.0 };
        let (one, two) = (timeline_order(1, cue), timeline_order(2, cue));
        assert_ne!(one.key, two.key);
        let mut faces = DialogueFaces::default();
        faces.follow(Some(&view(&[face("hero", "happy")])), 1);
        let said = faces.held().next().unwrap().1;
        assert!(said.key != one.key && said.key & FROM_TIMELINE == 0 && one.key & FROM_DIALOGUE == 0);
    }
}
