//! A document running as a screen: layout at three aspects, text from keys in two languages, bound
//! values, what a wrong document shows, and the quads a look makes.

mod common;

use common::{Model, Player, at, document, fonts, frame, glyphs, localizer, metrics};
use sapphire_assets::ui::{self, UiDocument};
use sapphire_look::{UiDrawList, UiPageState, UiQuad};
use sapphire_math::Vec2;
use sapphire_ui::{Atlas, Insets, PROBLEM_COLOR, UiFrame, UiMetrics, UiTree};

const STYLES: &str = r#"styles: [
    (name: "label", role: "body", size: 16),
    (name: "title", role: "display", size: 32),
],"#;

fn run_once(data: &str, model: &Model, language: &str, size: [f32; 2]) -> (UiTree, UiDrawList) {
    let mut tree = UiTree::new(&document(data)).unwrap();
    let mut list = UiDrawList::default();
    frame(&mut tree, &mut fonts(), &localizer(language), model, metrics(size), &Atlas::paint(), &mut list);
    (tree, list)
}

fn drawn_with(list: &UiDrawList, color: [u8; 4]) -> Vec<&UiQuad> {
    list.quads.iter().filter(|quad| quad.color == color).collect()
}

#[test]
fn the_interface_s_type_scale_is_the_cook_s() {
    assert_eq!(ui::TYPE_SCALE, sapphire_text::TYPE_SCALE);
}

#[test]
fn a_screen_holds_its_anchors_at_each_aspect() {
    let data = r#"nodes: [
        (name: "root", layout: (direction: Row, justify: SpaceBetween)),
        (name: "left", parent: "root", layout: (width: Points(100), height: Percent(100))),
        (name: "right", parent: "root", layout: (width: Percent(25), height: Points(50))),
        (name: "corner", parent: "root", layout: (
            width: Points(40), height: Points(20),
            anchored: (right: Points(10), bottom: Points(5)),
        )),
    ],"#;
    for size in [[1024.0, 768.0], [1280.0, 720.0], [1680.0, 720.0]] {
        let (tree, _) = run_once(data, &Model::default(), "en", size);
        let rect = |name| tree.rect(tree.find(name).unwrap());
        assert_eq!(rect("root").max, Vec2::from(size));
        assert_eq!(rect("left").max, Vec2::new(100.0, size[1]));
        assert_eq!(rect("right").max.x, size[0]);
        assert_eq!(rect("right").width(), size[0] * 0.25);
        assert_eq!(rect("corner").max, Vec2::new(size[0] - 10.0, size[1] - 5.0));
    }
}

#[test]
fn an_anchored_node_is_placed_against_its_own_parent_inside_an_anchored_one() {
    // A box behind a centered band, the band inside a strip anchored to the frame's foot (M4 A2's subtitles).
    let data = r#"nodes: [
        (name: "root"),
        (name: "strip", parent: "root", layout: (
            anchored: (left: Points(0), right: Points(0), bottom: Points(40)), direction: Column, align: Center,
        )),
        (name: "band", parent: "strip", layout: (width: Points(300), height: Points(60), padding: (left: 24))),
        (name: "box", parent: "band", layout: (anchored: (left: Points(0), right: Points(0), top: Points(0), bottom: Points(0)))),
    ],"#;
    let (tree, _) = run_once(data, &Model::default(), "en", [1280.0, 720.0]);
    let rect = |name| tree.rect(tree.find(name).unwrap());
    assert_eq!(rect("band").min, Vec2::new(490.0, 620.0));
    assert_eq!(rect("box"), rect("band"));
}

#[test]
fn the_safe_area_moves_what_asks_for_it() {
    let doc = document(
        r#"nodes: [
        (name: "root"),
        (name: "safe", parent: "root", layout: (safe_area: true, grow: 1)),
        (name: "inner", parent: "safe", layout: (width: Points(10), height: Points(10))),
    ],"#,
    );
    let mut tree = UiTree::new(&doc).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let notch =
        UiMetrics::new(Vec2::new(800.0, 400.0), Insets { left: 44.0, right: 0.0, top: 12.0, bottom: 0.0 });
    frame(&mut tree, &mut fonts, &loc, &Model::default(), notch, &atlas, &mut list);
    assert_eq!(tree.rect(tree.find("inner").unwrap()).min, Vec2::new(44.0, 12.0));
    // A device that turns moves it, with nothing else changed.
    let turned = UiMetrics { insets: Insets::default(), ..notch };
    frame(&mut tree, &mut fonts, &loc, &Model::default(), turned, &atlas, &mut list);
    assert_eq!(tree.rect(tree.find("inner").unwrap()).min, Vec2::ZERO);
}

#[test]
fn a_text_box_sizes_from_its_string_in_the_language_in_force() {
    let data = format!(
        r#"{STYLES} nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "title", parent: "root", text: (key: "title", style: "title", color: (255, 255, 255, 255))),
        (name: "under", parent: "root", layout: (width: Points(10), height: Points(10))),
    ],"#
    );
    let mut tree = UiTree::new(&document(&data)).unwrap();
    let (mut fonts, mut loc, atlas, mut list) =
        (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    frame(&mut tree, &mut fonts, &loc, &Model::default(), metrics([800.0, 600.0]), &atlas, &mut list);
    let rect = |tree: &UiTree, name| tree.rect(tree.find(name).unwrap());
    let english = rect(&tree, "title");
    assert_eq!(tree.strings().next(), Some(("title", "Main Menu")));
    assert!(english.width() > 100.0, "{english:?}");
    assert_eq!(rect(&tree, "under").min.y, english.max.y, "what follows stacks under the text");
    assert!(glyphs(&list).next().is_some());

    loc.set_text_locale("ja").unwrap();
    frame(&mut tree, &mut fonts, &loc, &Model::default(), metrics([800.0, 600.0]), &atlas, &mut list);
    assert_eq!(tree.strings().next(), Some(("title", "メインメニュー")));
    assert_ne!(rect(&tree, "title").width(), english.width());
    assert_eq!(list.tree.layouts, 1);
    assert_eq!(list.tree.resolves, 1);
}

#[test]
fn a_wrapped_text_wraps_inside_its_padding_and_pushes_what_follows() {
    let data = format!(
        r#"{STYLES} nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "para", parent: "root",
            layout: (width: Points(120), padding: (left: 10, right: 10, top: 6, bottom: 6)),
            text: (key: "only-english", style: "label", color: (255, 255, 255, 255), wrap: true)),
        (name: "under", parent: "root", layout: (width: Points(10), height: Points(10))),
    ],"#
    );
    let (tree, list) = run_once(&data, &Model::default(), "en", [800.0, 600.0]);
    let para = tree.rect(tree.find("para").unwrap());
    assert_eq!(para.width(), 120.0);
    // A line of this runs past 100 points, so more than one, inside 12 points of padding.
    assert!(para.height() > 12.0 + 2.0 * 16.0, "{para:?}");
    assert_eq!(tree.rect(tree.find("under").unwrap()).min.y, para.max.y);
    for glyph in glyphs(&list) {
        assert!(glyph.origin[0] >= 9.0 && glyph.origin[0] + glyph.axis_x[0] <= 111.0, "{glyph:?}");
        assert!(glyph.origin[1] >= 5.0, "{glyph:?}");
    }
}

#[test]
fn an_unchanged_model_costs_no_work_and_a_change_only_its_own() {
    let data = format!(
        r#"model: "m.model.ron", {STYLES} nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "left", parent: "root", text: (
            key: "enemies-left", style: "label", color: (255, 255, 255, 255),
            args: [(name: "count", path: "player.enemies")],
        )),
        (name: "bar", parent: "root",
            layout: (width: Points(100), height: Points(10)),
            look: (fill: Color((1, 2, 3, 255))),
            binds: [(to: Fill, path: "player.health")]),
    ],"#
    );
    let mut tree = UiTree::new(&document(&data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let mut model = Model { player: Player { health: 1.0, enemies: 3, shown: true }, ..Model::default() };
    let size = metrics([800.0, 600.0]);
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(tree.strings().next(), Some(("left", "3 enemies left")));
    assert_eq!(list.tree.layouts, 1);
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(
        (list.tree.layouts, list.tree.resolves, list.tree.bound_changes, list.tree.shaped),
        (0, 0, 0, 0),
        "the same values again",
    );

    // A number that moves resolves one string, and lays out since the string is new.
    model.player.enemies = 1;
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(tree.strings().next(), Some(("left", "One enemy left")));
    assert_eq!((list.tree.layouts, list.tree.resolves, list.tree.bound_changes), (1, 1, 1));

    // A fill that moves paints.
    model.player.health = 0.25;
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!((list.tree.layouts, list.tree.resolves, list.tree.bound_changes), (0, 0, 1));
    let bar = drawn_with(&list, [1, 2, 3, 255])[0];
    assert_eq!(bar.clip, [0.0, 0.25]);
}

#[test]
fn shown_takes_a_node_out_of_the_layout_with_its_children() {
    let data = r#"model: "m.model.ron", nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "panel", parent: "root",
            layout: (width: Points(50), height: Points(50)),
            look: (fill: Color((9, 9, 9, 255))),
            binds: [(to: Shown, path: "player.shown")]),
        (name: "in-panel", parent: "panel",
            layout: (width: Points(10), height: Points(10)),
            look: (fill: Color((8, 8, 8, 255)))),
        (name: "under", parent: "root", layout: (width: Points(10), height: Points(10))),
    ],"#;
    let mut tree = UiTree::new(&document(data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let size = metrics([800.0, 600.0]);
    let mut model = Model { player: Player { shown: true, ..Player::default() }, ..Model::default() };
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(tree.rect(tree.find("under").unwrap()).min.y, 50.0);
    assert_eq!(drawn_with(&list, [8, 8, 8, 255]).len(), 1);

    model.player.shown = false;
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(tree.rect(tree.find("under").unwrap()).min.y, 0.0);
    assert!(!tree.is_visible(tree.find("in-panel").unwrap()));
    assert!(drawn_with(&list, [9, 9, 9, 255]).is_empty());
    assert!(drawn_with(&list, [8, 8, 8, 255]).is_empty());
}

/// The player's text size (M4 A2): the tree's scale on every text, and a bound one on its own node's, which
/// lays out larger too. What can't be a scale reads as the nearest that can.
#[test]
fn text_grows_by_the_trees_scale_and_a_bound_one() {
    let data = format!(
        r#"{STYLES} model: "m.model.ron", nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "plain", parent: "root", text: (key: "title", style: "label", color: (255, 255, 255, 255))),
        (name: "bound", parent: "root", binds: [(to: TextScale, path: "player.health")],
            text: (key: "title", style: "label", color: (255, 255, 255, 255))),
    ],"#
    );
    let mut tree = UiTree::new(&document(&data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let size = metrics([800.0, 600.0]);
    let mut model = Model { player: Player { health: 1.0, ..Player::default() }, ..Model::default() };
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    let pixels = |tree: &UiTree| tree.text_pixels().map(|(_, pixels)| pixels).collect::<Vec<_>>();
    let width = |tree: &UiTree| tree.rect(tree.find("bound").unwrap()).width();
    let [base, same] = pixels(&tree)[..] else { panic!("two texts") };
    assert_eq!(base, same);
    let narrow = width(&tree);
    let mut times = |tree: &mut UiTree, model: &Model| {
        frame(tree, &mut fonts, &loc, model, size, &atlas, &mut list);
        pixels(tree).iter().map(|pixels| pixels / base).collect::<Vec<_>>()
    };
    model.player.health = 2.0;
    assert_eq!(times(&mut tree, &model), [1.0, 2.0]);
    assert!(width(&tree) > narrow * 1.8, "{} against {narrow}", width(&tree));
    tree.set_text_scale(1.5);
    assert_eq!(times(&mut tree, &model), [1.5, 3.0]);
    model.player.health = f32::NAN;
    assert_eq!(times(&mut tree, &model), [1.5, 1.5]);
    model.player.health = 100.0;
    assert_eq!(times(&mut tree, &model), [1.5, 6.0]);
    tree.set_text_scale(1.0);
    model.player.health = 1.0;
    assert_eq!(times(&mut tree, &model), [1.0, 1.0]);
    assert_eq!(width(&tree), narrow, "back as it was");
}

#[test]
fn opacity_multiplies_down_the_tree() {
    let data = r#"model: "m.model.ron", nodes: [
        (name: "root"),
        (name: "panel", parent: "root", opacity: 0.5,
            layout: (width: Points(50), height: Points(50)),
            binds: [(to: Opacity, path: "player.health")]),
        (name: "in-panel", parent: "panel",
            layout: (width: Points(10), height: Points(10)),
            look: (fill: Color((200, 100, 50, 200)))),
    ],"#;
    let model = Model { player: Player { health: 0.5, ..Player::default() }, ..Model::default() };
    let (_, list) = run_once(data, &model, "en", [800.0, 600.0]);
    assert_eq!(list.quads[0].color, [200, 100, 50, 50], "{:?}", list.quads[0]);
}

#[test]
fn a_model_of_dynamic_values_reads_once_its_shape_is_declared() {
    use bevy_reflect::Typed;
    use bevy_reflect::structs::DynamicStruct;
    let data = r#"model: "m.model.ron", nodes: [
        (name: "root"),
        (name: "panel", parent: "root",
            layout: (width: Points(50), height: Points(50)),
            look: (fill: Color((200, 100, 50, 200))),
            binds: [(to: Opacity, path: "player.health")]),
    ],"#;
    let mut player = DynamicStruct::default();
    player.insert("health", 0.5_f64);
    let mut model = DynamicStruct::default();
    model.insert("player", player);
    let mut tree = UiTree::new(&document(data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let mut run = |tree: &mut UiTree, list: &mut UiDrawList, with_model: bool| {
        list.reset();
        fonts.begin_frame(UiPageState::NO_RENDERER, &loc);
        let model = with_model.then_some(&model as &dyn bevy_reflect::PartialReflect);
        let frame = UiFrame { metrics: metrics([800.0, 600.0]), now: at(0.0), loc: &loc, model };
        tree.update(&frame, &mut fonts);
        tree.draw(&mut fonts, &atlas, list);
    };
    run(&mut tree, &mut list, true);
    assert_eq!(tree.problems().len(), 1, "no type to check against: {:#?}", tree.problems());
    tree.declare_model(ui::UiModel::of_type(Model::type_info()));
    // A frame with no model first: the dynamic one, which has no type either, is still checked.
    run(&mut tree, &mut list, false);
    assert_eq!(tree.problems().len(), 1, "no model to read: {:#?}", tree.problems());
    run(&mut tree, &mut list, true);
    assert_eq!(tree.problems(), &[]);
    assert_eq!(drawn_with(&list, [200, 100, 50, 100]).len(), 1, "{:?}", list.quads);
}

#[test]
fn a_path_the_model_lacks_is_something_you_can_see() {
    let data = format!(
        r#"model: "m.model.ron", {STYLES} nodes: [
        (name: "root"),
        (name: "bar", parent: "root",
            layout: (width: Points(100), height: Points(10)),
            binds: [(to: Fill, path: "player.mana")]),
        (name: "named", parent: "root", text: (
            key: "welcome", style: "label", color: (255, 255, 255, 255),
            args: [(name: "name", path: "player.nickname")],
        )),
        (name: "kind", parent: "root",
            layout: (width: Points(100), height: Points(10)),
            binds: [(to: Shown, path: "player.health")]),
    ],"#
    );
    let (tree, list) = run_once(&data, &Model::default(), "en", [800.0, 600.0]);
    let fields: Vec<_> = tree.problems().iter().map(|problem| (&*problem.node, &*problem.field)).collect();
    assert_eq!(
        fields,
        [("bar", "binds[0].path"), ("named", "text.args[0].path"), ("kind", "binds[0].path")],
        "{:#?}",
        tree.problems(),
    );
    assert!(tree.problems()[0].problem.contains("player.mana"), "{:#?}", tree.problems());
    assert_eq!(list.tree.problems, 3);
    assert_eq!(drawn_with(&list, PROBLEM_COLOR).len(), 3 * 4, "a frame around each");
    let (_, string) = tree.strings().next().unwrap();
    assert!(string.contains("{player.nickname}"), "{string}");
}

#[test]
fn a_tree_binding_with_no_model_says_so() {
    let data = r#"model: "m.model.ron", nodes: [
        (name: "root", binds: [(to: Shown, path: "player.shown")]),
    ],"#;
    let mut tree = UiTree::new(&document(data)).unwrap();
    let (mut fonts, loc) = (fonts(), localizer("en"));
    fonts.begin_frame(UiPageState::NO_RENDERER, &loc);
    tree.update(
        &UiFrame { metrics: metrics([10.0, 10.0]), now: at(0.0), loc: &loc, model: None },
        &mut fonts,
    );
    assert!(tree.problems()[0].problem.contains("no model"), "{:#?}", tree.problems());
}

#[test]
fn a_value_absent_right_now_is_not_a_problem() {
    let data = r#"model: "m.model.ron", nodes: [
        (name: "root"),
        (name: "bar", parent: "root",
            layout: (width: Points(100), height: Points(10)),
            look: (fill: Color((4, 4, 4, 255))),
            binds: [(to: Fill, path: "stance.0")]),
        (name: "third", parent: "root",
            layout: (width: Points(100), height: Points(10)),
            look: (fill: Color((5, 5, 5, 255))),
            binds: [(to: Fill, path: "party[2].health")]),
    ],"#;
    let mut model = Model::default();
    let (tree, list) = run_once(data, &model, "en", [800.0, 600.0]);
    assert_eq!(tree.problems(), &[]);
    assert!(drawn_with(&list, [4, 4, 4, 255]).is_empty(), "nothing in the Option: no fill");
    assert!(drawn_with(&list, [5, 5, 5, 255]).is_empty(), "past the list's end: no fill");
    model.stance = Some(1.0);
    model.party = vec![Player::default(), Player::default(), Player { health: 1.0, ..Player::default() }];
    let (tree, list) = run_once(data, &model, "en", [800.0, 600.0]);
    assert_eq!(tree.problems(), &[]);
    assert_eq!(drawn_with(&list, [4, 4, 4, 255]).len(), 1);
    assert_eq!(drawn_with(&list, [5, 5, 5, 255]).len(), 1);
}

#[test]
fn a_role_no_chain_answers_is_a_problem() {
    let data = r#"styles: [(name: "odd", role: "subtitle", size: 16)], nodes: [
        (name: "root", text: (key: "title", style: "odd", color: (255, 255, 255, 255))),
    ],"#;
    let (tree, list) = run_once(data, &Model::default(), "en", [100.0, 100.0]);
    assert_eq!(tree.problems()[0].field, "text.style");
    assert!(tree.problems()[0].problem.contains("subtitle"), "{:#?}", tree.problems());
    assert_eq!(drawn_with(&list, PROBLEM_COLOR).len(), 4);
    assert_eq!(glyphs(&list).count(), 0);
}

#[test]
fn pseudo_localization_finds_what_isn_t_localized() {
    let data = format!(
        r#"{STYLES} nodes: [
        (name: "root", layout: (direction: Column)),
        (name: "good", parent: "root", text: (key: "title", style: "label", color: (255, 255, 255, 255))),
        (name: "missing", parent: "root", text: (key: "no-such-key", style: "label", color: (255, 255, 255, 255))),
    ],"#
    );
    let mut tree = UiTree::new(&document(&data)).unwrap();
    let (mut fonts, mut loc, atlas, mut list) =
        (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    loc.set_pseudo(true);
    frame(&mut tree, &mut fonts, &loc, &Model::default(), metrics([800.0, 600.0]), &atlas, &mut list);
    let unframed: Vec<_> = tree.unframed().map(|(node, _)| node).collect();
    assert_eq!(unframed, ["missing"]);
}

#[test]
fn nodes_draw_parent_first_then_each_subtree_as_the_document_lists_it() {
    let data = r#"nodes: [
        (name: "root"),
        (name: "a", parent: "root", layout: (width: Points(9), height: Points(9)), look: (fill: Color((1, 0, 0, 255)))),
        (name: "b", parent: "root", layout: (width: Points(9), height: Points(9)), look: (fill: Color((2, 0, 0, 255)))),
        (name: "a-child", parent: "a", layout: (width: Points(3), height: Points(3)), look: (fill: Color((3, 0, 0, 255)))),
    ],"#;
    let (_, list) = run_once(data, &Model::default(), "en", [100.0, 100.0]);
    let order: Vec<u8> = list.quads.iter().map(|quad| quad.color[0]).collect();
    assert_eq!(order, [1, 3, 2]);
}

#[test]
fn a_nine_slice_keeps_its_corners_and_shares_its_edges() {
    let data = r#"nodes: [
        (name: "root"),
        (name: "box", parent: "root",
            layout: (width: Points(301.4), height: Points(80), margin: (left: 10.3, top: 20.2)),
            look: (shape: Slice(sprite: Button, corner: 12), fill: Color((7, 7, 7, 255)))),
    ],"#;
    let (_, list) = run_once(data, &Model::default(), "en", [800.0, 600.0]);
    let pieces = drawn_with(&list, [7, 7, 7, 255]);
    assert_eq!(pieces.len(), 9);
    // Pieces share their edges, with no gap for a pixel to show through.
    for row in pieces.chunks(3) {
        for pair in row.windows(2) {
            assert_eq!(pair[0].origin[0] + pair[0].axis_x[0], pair[1].origin[0], "{pair:?}");
        }
        // The middle column reads one column of texels, stretched.
        assert_eq!(row[1].uv[0], row[1].uv[2]);
    }
    for column in 0..3 {
        for row in 0..2 {
            let (upper, lower) = (pieces[row * 3 + column], pieces[(row + 1) * 3 + column]);
            assert_eq!(upper.origin[1] + upper.axis_y[1], lower.origin[1]);
        }
    }
    // The corners read the same size however long the box is.
    let shorter = data.replace("301.4", "90");
    let (_, list) = run_once(&shorter, &Model::default(), "en", [800.0, 600.0]);
    assert_eq!(drawn_with(&list, [7, 7, 7, 255])[0].axis_x[0], pieces[0].axis_x[0]);
}

#[test]
fn a_gradient_runs_across_a_nine_slice_s_pieces_as_one() {
    let data = r#"nodes: [
        (name: "root"),
        (name: "box", parent: "root",
            layout: (width: Points(300), height: Points(100)),
            look: (shape: Slice(sprite: Button, corner: 20),
                   fill: Gradient(from: (0, 0, 0, 255), to: (255, 255, 255, 255), angle: 0))),
    ],"#;
    let (_, list) = run_once(data, &Model::default(), "en", [800.0, 600.0]);
    assert_eq!(list.quads.len(), 9);
    let share =
        |quad: &UiQuad, x: f32, y: f32| quad.gradient[0] * x + quad.gradient[1] * y + quad.gradient[2];
    for row in list.quads.chunks(3) {
        for pair in row.windows(2) {
            let (left, right) =
                (share(pair.first().unwrap(), 1.0, 0.5), share(pair.last().unwrap(), 0.0, 0.5));
            assert!((left - right).abs() < 1e-5, "{left} then {right}");
        }
        // Nothing at the box's left edge, which isn't the quad's: that runs past for the field.
        let local = -row[0].origin[0] / row[0].axis_x[0];
        assert!(share(&row[0], local, 0.5).abs() < 1e-5);
        assert!((share(&row[2], 1.0, 0.5) - (300.0 - row[2].origin[0]) / 300.0).abs() < 1.0);
    }
    assert_eq!(list.quads[0].color_end, [255, 255, 255, 255]);
}

#[test]
fn a_mask_holds_what_s_under_it_and_what_s_past_it_doesn_t_draw() {
    let data = r#"nodes: [
        (name: "root", layout: (align: Start)),
        (name: "window", parent: "root", mask: true,
            layout: (width: Points(100), height: Points(50), direction: Column)),
        (name: "inside", parent: "window",
            layout: (width: Points(10), height: Points(40), shrink: 0),
            look: (fill: Color((1, 1, 1, 255)))),
        (name: "across", parent: "window",
            layout: (width: Points(10), height: Points(40), shrink: 0),
            look: (fill: Color((2, 2, 2, 255)))),
        (name: "past", parent: "window",
            layout: (width: Points(10), height: Points(40), shrink: 0),
            look: (fill: Color((3, 3, 3, 255)))),
        (name: "outside", parent: "root",
            layout: (width: Points(10), height: Points(40)),
            look: (fill: Color((4, 4, 4, 255)))),
    ],"#;
    let (_, list) = run_once(data, &Model::default(), "en", [800.0, 600.0]);
    assert_eq!(drawn_with(&list, [1, 1, 1, 255]).len(), 1);
    assert_eq!(drawn_with(&list, [2, 2, 2, 255]).len(), 1);
    assert!(drawn_with(&list, [3, 3, 3, 255]).is_empty(), "wholly past the mask: no quad");
    assert_eq!(drawn_with(&list, [4, 4, 4, 255]).len(), 1);
    let masks: Vec<_> = list.masks.iter().map(|mask| (mask.first, mask.rect)).collect();
    assert_eq!(masks, [(0, Some([0.0, 0.0, 100.0, 50.0])), (2, None)]);
}

#[test]
fn a_document_that_fails_its_checks_makes_no_tree() {
    let mut bad: UiDocument = document(r#"nodes: [(name: "root")],"#);
    bad.nodes[0].opacity = 2.0;
    assert!(UiTree::new(&bad).is_err());
    bad.nodes.clear();
    assert!(UiTree::new(&bad).is_err());
}

/// A string too long for a row, and what a scroller lets run on.
#[test]
fn it_says_which_text_leaves_what_holds_it() {
    let run = |row_width: u32, scroll: &str| {
        let data = format!(
            r#"styles: [(name: "body", role: "body", size: 16)], nodes: [
                (name: "root", layout: (direction: Column)),
                (name: "list", parent: "root", {scroll} layout: (direction: Column, height: Points(40))),
                (
                    name: "row",
                    parent: "list",
                    layout: (width: Points({row_width}), height: Points(100), shrink: 0),
                    text: (key: "title", style: "body", color: (255, 255, 255, 255)),
                ),
                (name: "box", parent: "root", layout: (direction: Row, width: Points(8), height: Points(30))),
                (name: "label", parent: "box", text: (key: "title", style: "body", color: (255, 255, 255, 255))),
            ]"#
        );
        let (tree, _) = run_once(&data, &Model::default(), "en", [320.0, 240.0]);
        tree.overflowing().into_iter().map(|(node, left)| format!("{node} leaves {left}")).collect::<Vec<_>>()
    };
    let (row, label) = ("row leaves list", "label leaves box");
    assert_eq!(run(100, ""), [row, label], "too tall for its list, and too wide for its box");
    assert_eq!(run(100, "mask: true, scroll: Vertical,"), [label], "a scroller lets it run on");
    assert_eq!(run(400, "mask: true, scroll: Vertical,"), [row, label], "but not across");
}
