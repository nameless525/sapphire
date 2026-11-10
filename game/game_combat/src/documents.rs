//! The numbers a game's fight starts on: Sapphire's compiled-in documents, or what the game named in its
//! manifest (`sapphire_assets::documents`), read here whole at its start. What a cook could only check by its
//! envelope, this reads: a document that's missing or won't read stops the game and says which, rather than
//! leaving it on numbers nobody chose.
//!
//! A game's numbers need no Rust. [`Fight`](crate::fight::Fight) stands a fight on them, and what isn't the
//! fight's reads its own: the HUD's design, the bindings.

use sapphire_anim::{BodyTuning, CharacterMotion};
use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, NamedDocument, SHIPPED_DIR, shipped_path};
use sapphire_gameplay::replay::LiveMoves;
use sapphire_gameplay::targeting::TargetingTuning;
use sapphire_input::InputBindings;
use sapphire_platform::ContentFs;
use sapphire_ui::HudDesign;
use sapphire_vfx::EffectLibrary;

/// What a fight starts on. `Default` is [`Self::shipped`].
#[derive(Clone, Debug)]
pub struct FightDocuments {
    /// Who a soft lock picks, in the simulation (and so in a replay's hash).
    pub targeting: TargetingTuning,
    /// How bodies place their feet and turn their heads: in the simulation, and so in a replay's hash, and in
    /// presentation's IK.
    pub body: BodyTuning,
    /// Hair, cloth and how much of the IK shows.
    pub motion: CharacterMotion,
    /// Where the combat HUD's pieces go.
    pub hud: HudDesign,
    /// Combat's bindings: `None` is `sapphire_controls`' own.
    pub bindings: Option<InputBindings>,
    /// What cues and sword trails play: `None` is `sapphire_vfx`'s own.
    pub effects: Option<EffectLibrary>,
}

impl Default for FightDocuments {
    fn default() -> Self {
        Self::shipped()
    }
}

impl FightDocuments {
    /// Sapphire's own.
    #[must_use]
    pub fn shipped() -> Self {
        Self {
            targeting: TargetingTuning::shipped(),
            body: *BodyTuning::shipped(),
            motion: *CharacterMotion::shipped(),
            hud: *HudDesign::shipped(),
            bindings: None,
            effects: None,
        }
    }

    /// What the content an app ships says: the manifest a cook writes, and what it names.
    pub fn read(content: &ContentFs) -> Result<Self, String> {
        Self::read_with(&|path| content.read(path).map_err(|error| error_chain(&error)))
    }

    /// [`Self::read`], with `read` giving a file's bytes from the content's root.
    pub fn read_with(read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Self, String> {
        let manifest_path = format!("{SHIPPED_DIR}/{MANIFEST_FILE}");
        let manifest = read(&manifest_path).map_err(|error| {
            format!(
                "{error}: a cook writes the game's manifest, so this content is older than that: cook again"
            )
        })?;
        let manifest = GameDocuments::from_ron(&String::from_utf8_lossy(&manifest))
            .map_err(|error| format!("{manifest_path}: {}", error_chain(&error)))?;

        let mut documents = Self::shipped();
        for named in manifest.named() {
            let text = read(&shipped_path(named.path))
                .and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
                .map_err(|error| because(&named, &error))?;
            let bad = |error: &dyn std::error::Error| because(&named, &error_chain(error));
            match named.field {
                "targeting" => {
                    documents.targeting = TargetingTuning::from_ron(&text).map_err(|error| bad(&error))?;
                }
                "body" => documents.body = BodyTuning::from_ron(&text).map_err(|error| bad(&error))?,
                "motion" => {
                    documents.motion = CharacterMotion::from_ron(&text).map_err(|error| bad(&error))?
                }
                "hud" => documents.hud = HudDesign::from_ron(&text).map_err(|error| bad(&error))?,
                "bindings" => {
                    let bindings = sapphire_serialize::from_ron(&text).map_err(|error| bad(&error))?;
                    documents.bindings = Some(bindings);
                }
                "effects" => {
                    documents.effects = Some(EffectLibrary::from_ron(&text).map_err(|error| bad(&error))?);
                }
                // The flow's (`game_flow`) and the screens' (`game_ui`), which read them themselves: here only
                // that they're there.
                "flow" | "flags" | "menus" => {}
                field => return Err(format!("{manifest_path}: nothing here reads `{field}`")),
            }
        }
        Ok(documents)
    }

    /// What a fight starts from, for [`Combat`](crate::combat::Combat): its simulation holds the weights and
    /// the body table.
    #[must_use]
    pub fn live(&self) -> LiveMoves {
        let mut live = LiveMoves::default();
        live.set_targeting(self.targeting);
        live.set_bodies(self.body);
        live
    }
}

/// An error and its sources, as one line.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Says which document's wrong, and what's wrong with it.
fn because(named: &NamedDocument<'_>, error: &str) -> String {
    format!("the game's `{}` document, {}: {error}", named.field, shipped_path(named.path))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sapphire_gameplay::replay::{Recording, SCENARIOS, find_scenario};
    use sapphire_math::Vec2;
    use sapphire_reflect::stable_name;
    use sapphire_ui::{HudLayout, Insets, UiMetrics};

    use super::*;

    const TARGETING: &str = include_str!("../../../crates/sapphire_gameplay/default_targeting.ron");
    const BODY: &str = include_str!("../../../crates/sapphire_body_pose/default_body.ron");
    const MOTION: &str = include_str!("../../../crates/sapphire_anim/default_character_motion.ron");
    const HUD: &str = include_str!("../../../crates/sapphire_ui/default_hud.ron");
    const BINDINGS: &str = include_str!("../../../crates/sapphire_controls/combat_bindings.ron");
    const EFFECTS: &str = include_str!("../../../crates/sapphire_vfx/default_effects.ron");

    /// Content with `manifest`'s fields, and `files` under the documents' directory.
    fn content(manifest: &str, files: &[(&str, String)]) -> HashMap<String, Vec<u8>> {
        let text = format!("(type: \"sapphire::assets::GameDocuments\", version: 1, data: ({manifest}))");
        let mut content = HashMap::from([(format!("{SHIPPED_DIR}/{MANIFEST_FILE}"), text.into_bytes())]);
        content.extend(files.iter().map(|(path, text)| (shipped_path(path), text.clone().into_bytes())));
        content
    }

    fn read(content: &HashMap<String, Vec<u8>>) -> Result<FightDocuments, String> {
        FightDocuments::read_with(&|path| content.get(path).cloned().ok_or_else(|| format!("no {path}")))
    }

    /// The cooker can't link these types, so it holds their names as text, and this holds them to the types.
    #[test]
    fn the_manifests_names_are_these_types() {
        let every = GameDocuments {
            targeting: Some(String::new()),
            body: Some(String::new()),
            motion: Some(String::new()),
            hud: Some(String::new()),
            bindings: Some(String::new()),
            effects: Some(String::new()),
            // The flow's own: `game_flow` holds the flow's name, and the flags' joins it when Q2's type lands.
            flow: None,
            flags: None,
            // The screens' own: `game_ui` holds their name.
            menus: None,
        };
        let names: Vec<&str> = every.named().map(|named| named.type_name).collect();
        assert_eq!(
            names,
            [
                stable_name::<TargetingTuning>(),
                stable_name::<BodyTuning>(),
                stable_name::<CharacterMotion>(),
                stable_name::<HudDesign>(),
                stable_name::<InputBindings>(),
                stable_name::<EffectLibrary>(),
            ]
        );
    }

    /// With an empty manifest a game is on Sapphire's numbers, and with no manifest it isn't anywhere: that's
    /// content from before cooks wrote one, and it says to cook again.
    #[test]
    fn an_empty_manifest_is_sapphires_numbers_and_none_is_old_content() {
        let documents = read(&content("", &[])).expect("reads");
        assert_eq!(documents.targeting, TargetingTuning::shipped());
        assert_eq!(documents.body, *BodyTuning::shipped());
        assert_eq!(documents.motion, *CharacterMotion::shipped());
        assert_eq!(documents.hud, *HudDesign::shipped());
        assert!(documents.bindings.is_none() && documents.effects.is_none());
        let refused = read(&HashMap::new()).expect_err("no manifest");
        assert!(refused.contains("cook again"), "{refused}");
    }

    /// What pillar 1 asks of these numbers: a game with its own has a tree of documents and no Rust, and each
    /// reaches what reads it, the simulation's going into its replay hash.
    #[test]
    fn a_game_with_its_own_documents_needs_no_rust() {
        let own = content(
            "targeting: Some(\"my_targeting.ron\"), body: Some(\"look/my_body.ron\"), \
             motion: Some(\"look/my_motion.ron\"), hud: Some(\"my_hud.ron\"), \
             bindings: Some(\"my_bindings.ron\"), effects: Some(\"my_effects.ron\")",
            &[
                ("my_targeting.ron", TARGETING.replacen("range: 12.0", "range: 15.0", 1)),
                ("look/my_body.ron", BODY.replacen("rate: 4.5", "rate: 5.0", 1)),
                ("look/my_motion.ron", MOTION.replacen("root_inertia: 0.25", "root_inertia: 0.5", 1)),
                ("my_hud.ron", HUD.replacen("margin: 16.0", "margin: 30.0", 1)),
                ("my_bindings.ron", BINDINGS.to_owned()),
                ("my_effects.ron", EFFECTS.to_owned()),
            ],
        );
        let documents = read(&own).expect("reads");
        assert_eq!(documents.targeting.soft_lock.range, 15.0);
        assert_eq!(documents.body.look.rate, 5.0);
        assert_eq!(documents.motion.springs.root_inertia, 0.5);
        assert!(documents.bindings.is_some() && documents.effects.is_some());

        let metrics = UiMetrics::new(Vec2::new(874.0, 402.0), Insets::default());
        let moved = HudLayout::designed(metrics, &documents.hud);
        assert_ne!(moved.attack.center, HudLayout::new(metrics).attack.center);

        let scenario = find_scenario(&SCENARIOS, "duel").expect("listed");
        let hash = |live: &LiveMoves| {
            let recording = Recording::tuned(scenario, scenario.config, live).expect("sets up");
            recording.simulation().tables_hash()
        };
        assert_ne!(hash(&documents.live()), hash(&LiveMoves::default()), "a replay can tell");
        let body_only = FightDocuments { body: documents.body, ..FightDocuments::shipped() };
        assert_ne!(hash(&body_only.live()), hash(&LiveMoves::default()), "the body table alone too");
        assert_eq!(hash(&FightDocuments::shipped().live()), hash(&LiveMoves::default()), "and only then");
    }

    #[test]
    fn what_a_manifest_names_that_isnt_there_or_doesnt_read_stops_the_game_by_name() {
        let cases = [
            ("hud: Some(\"gone.ron\")", vec![], "`hud`"),
            ("hud: Some(\"my.ron\")", vec![("my.ron", HUD.replacen("margin:", "marginn:", 1))], "marginn"),
            (
                "targeting: Some(\"my.ron\")",
                vec![("my.ron", TARGETING.replacen("range: 12.0", "range: -1.0", 1))],
                "soft_lock.range",
            ),
            (
                "body: Some(\"my.ron\")",
                vec![("my.ron", BODY.replacen("soften: 0.985", "soften: 2.0", 1))],
                "feet.soften",
            ),
            (
                "motion: Some(\"my.ron\")",
                vec![("my.ron", MOTION.replacen("look_weight: 1.0", "look_weight: 3.0", 1))],
                "look_weight",
            ),
            ("bindings: Some(\"my.ron\")", vec![("my.ron", "not bindings".to_owned())], "my.ron"),
            ("effects: Some(\"my.ron\")", vec![("my.ron", "not effects".to_owned())], "my.ron"),
            ("hud_two: Some(\"my.ron\")", vec![], "hud_two"),
        ];
        for (manifest, files, named) in cases {
            let refused = read(&content(manifest, &files)).expect_err(manifest);
            assert!(refused.contains(named), "{manifest}: {refused}");
        }
    }
}
