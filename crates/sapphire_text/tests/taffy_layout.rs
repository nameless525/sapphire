//! `taffy` sizes a box from shaped text: a Japanese paragraph wraps to the width its column gives it,
//! and its height comes back to push the next box down.

mod common;

use sapphire_text::{TextStyle, TextSystem};
use taffy::prelude::*;
use taffy::{LayoutInput, LayoutOutput};

/// What a text leaf holds.
#[derive(Clone, Copy)]
struct Label {
    text: &'static str,
    style: TextStyle<'static>,
}

const SCALE: f32 = 1.5;

fn lay_out(text: &mut TextSystem, labels: [Label; 2], column: f32) -> (Layout, Layout) {
    let mut tree: TaffyTree<Label> = TaffyTree::new();
    let first = tree.new_leaf_with_context(Style::DEFAULT, labels[0]).expect("a leaf");
    let second = tree.new_leaf_with_context(Style::DEFAULT, labels[1]).expect("a leaf");
    let root = tree
        .new_with_children(
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size { width: length(column), height: auto() },
                ..Style::DEFAULT
            },
            &[first, second],
        )
        .expect("a column");
    tree.compute_layout_with_measure(
        root,
        Size::MAX_CONTENT,
        |input: LayoutInput, _, label: Option<&mut Label>, _: &Style| {
            let Some(label) = label else { return LayoutOutput::HIDDEN };
            let width = input.known_dimensions.width.or(match input.available_space.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent => Some(0.0),
                AvailableSpace::MaxContent => None,
            });
            let shaped = text.shape(label.text, &label.style, SCALE, width);
            LayoutOutput::from_outer_size(Size {
                width: input.known_dimensions.width.unwrap_or(shaped.width / SCALE),
                height: input.known_dimensions.height.unwrap_or(shaped.height / SCALE),
            })
        },
    )
    .expect("a layout");
    (*tree.layout(first).expect("laid out"), *tree.layout(second).expect("laid out"))
}

#[test]
fn a_column_wraps_text_and_stacks_what_follows() {
    let mut text = TextSystem::new();
    text.register_font(common::read_font(&common::fonts_dir(), common::MPLUS1));
    let string =
        "嵐が過ぎた朝、港の鐘がふたつ鳴った。「まだ船は戻らないのか」と老人は海を見つめたまま尋ねた。";
    let next = "攻撃力";
    let families = common::FAMILY_BODY;
    let style = TextStyle { families, size: 14.0, weight: 400.0, line_height: 1.3, locale: Some("ja") };
    let labels = [Label { text: string, style }, Label { text: next, style }];

    let (wide_a, wide_b) = lay_out(&mut text, labels, 1000.0);
    let (narrow_a, narrow_b) = lay_out(&mut text, labels, 140.0);
    assert!(narrow_a.size.height >= 3.0 * wide_a.size.height, "{wide_a:?} {narrow_a:?}");
    assert!(narrow_a.size.width <= 140.0, "{narrow_a:?}");
    assert_eq!(wide_b.location.y, wide_a.size.height, "the next box starts where the text ends");
    assert_eq!(narrow_b.location.y, narrow_a.size.height, "and moves down with it");
}
