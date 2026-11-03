//! A document's layout as `taffy` reads it.

use sapphire_assets::ui::{UiAlign, UiCell, UiDirection, UiDisplay, UiJustify, UiLayout, UiLength, UiTrack};
use taffy::prelude::{
    AlignItems, Dimension, Display, FlexDirection, FlexWrap, GridPlacement, JustifyContent, LengthPercentage,
    LengthPercentageAuto, Line, Position, Rect as TaffyRect, Size as TaffySize, Style as TaffyStyle,
};
use taffy::style::{
    GridTemplateComponent, MaxTrackSizingFunction, MinTrackSizingFunction, TrackSizingFunction,
};
use taffy::style_helpers::{FromFr, FromLength, FromPercent, TaffyAuto, line, minmax};

use crate::layout::Insets;

fn dimension(length: UiLength) -> Dimension {
    match length {
        UiLength::Auto => Dimension::auto(),
        UiLength::Points(points) => Dimension::length(points),
        UiLength::Percent(percent) => Dimension::percent(percent / 100.0),
    }
}

fn inset(length: UiLength) -> LengthPercentageAuto {
    match length {
        UiLength::Auto => LengthPercentageAuto::auto(),
        UiLength::Points(points) => LengthPercentageAuto::length(points),
        UiLength::Percent(percent) => LengthPercentageAuto::percent(percent / 100.0),
    }
}

fn align(align: UiAlign) -> AlignItems {
    match align {
        UiAlign::Start => AlignItems::START,
        UiAlign::End => AlignItems::END,
        UiAlign::Center => AlignItems::CENTER,
        UiAlign::Stretch => AlignItems::STRETCH,
    }
}

fn track(track: UiTrack) -> GridTemplateComponent<String> {
    let single: TrackSizingFunction = match track {
        UiTrack::Auto => TrackSizingFunction::AUTO,
        UiTrack::Points(points) => TrackSizingFunction::from_length(points),
        UiTrack::Percent(percent) => TrackSizingFunction::from_percent(percent / 100.0),
        UiTrack::Fr(share) => minmax(MinTrackSizingFunction::AUTO, MaxTrackSizingFunction::from_fr(share)),
    };
    GridTemplateComponent::Single(single)
}

fn placement(start: u16, span: u16) -> Line<GridPlacement<String>> {
    Line { start: line(start as i16), end: taffy::style_helpers::span(span) }
}

/// `layout` as a style, with `insets` added to its padding.
pub(super) fn taffy_style(layout: &UiLayout, insets: Insets) -> TaffyStyle {
    let pad = |points: f32, more: f32| LengthPercentage::length(points + more.max(0.0));
    let (padding, margin) = (layout.padding, layout.margin);
    let mut style = TaffyStyle {
        display: match layout.display {
            UiDisplay::Flex => Display::Flex,
            UiDisplay::Grid => Display::Grid,
        },
        flex_direction: match layout.direction {
            UiDirection::Row => FlexDirection::Row,
            UiDirection::Column => FlexDirection::Column,
        },
        flex_wrap: if layout.wrap { FlexWrap::Wrap } else { FlexWrap::NoWrap },
        justify_content: match layout.justify {
            UiJustify::Start => JustifyContent::START,
            UiJustify::End => JustifyContent::END,
            UiJustify::Center => JustifyContent::CENTER,
            UiJustify::SpaceBetween => JustifyContent::SPACE_BETWEEN,
            UiJustify::SpaceAround => JustifyContent::SPACE_AROUND,
            UiJustify::SpaceEvenly => JustifyContent::SPACE_EVENLY,
        },
        align_items: layout.align.map(align).unwrap_or_default(),
        align_self: layout.align_self.map(align),
        gap: TaffySize {
            width: LengthPercentage::length(layout.gap),
            height: LengthPercentage::length(layout.gap),
        },
        padding: TaffyRect {
            left: pad(padding.left, insets.left),
            right: pad(padding.right, insets.right),
            top: pad(padding.top, insets.top),
            bottom: pad(padding.bottom, insets.bottom),
        },
        margin: TaffyRect {
            left: LengthPercentageAuto::length(margin.left),
            right: LengthPercentageAuto::length(margin.right),
            top: LengthPercentageAuto::length(margin.top),
            bottom: LengthPercentageAuto::length(margin.bottom),
        },
        size: TaffySize { width: dimension(layout.width), height: dimension(layout.height) },
        min_size: TaffySize { width: inset(layout.min_width), height: inset(layout.min_height) },
        max_size: TaffySize { width: inset(layout.max_width), height: inset(layout.max_height) },
        flex_grow: layout.grow,
        flex_shrink: layout.shrink,
        aspect_ratio: layout.aspect,
        grid_template_columns: layout.columns.iter().copied().map(track).collect(),
        grid_template_rows: layout.rows.iter().copied().map(track).collect(),
        ..TaffyStyle::default()
    };
    if let Some(anchors) = layout.anchored {
        style.position = Position::Absolute;
        style.inset = TaffyRect {
            left: inset(anchors.left),
            right: inset(anchors.right),
            top: inset(anchors.top),
            bottom: inset(anchors.bottom),
        };
    }
    if let Some(UiCell { column, row, column_span, row_span }) = layout.cell {
        style.grid_column = placement(column, column_span);
        style.grid_row = placement(row, row_span);
    }
    style
}
