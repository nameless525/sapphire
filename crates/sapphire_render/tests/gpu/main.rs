//! GPU integration tests share one executable so a renderer edit links the large renderer and
//! its dependencies once, instead of once per suite. Each test still owns its GPU context and
//! fixtures; no device or renderer is shared between tests.
//!
//! Run all suites with
//! `cargo test -p sapphire_render --test gpu -- --ignored --test-threads=1`, or select one with
//! `cargo test -p sapphire_render --test gpu ambient:: -- --ignored --test-threads=1`.

mod ambient;
mod character;
mod character_shading;
mod cutouts;
mod debug_view;
mod environment;
mod eyes;
mod face;
mod fog;
mod foliage;
mod graph;
mod hot_reload;
mod layers;
mod outline;
mod render;
mod render_scale;
mod rim_hair;
mod shadows;
mod temporal;
mod texture;
mod view_culling;
