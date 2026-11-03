//! Paths over a zone's navmesh, inside the fixed step.
//!
//! An enemy's movement is checksummed state, so its path is too: everything here runs in the
//! simulation, reads no clock and touches no heap once built.
//!
//! # Owns
//!
//! - **The graph** ([`graph`]): [`NavGraph`], a cooked mesh as world positions and flat lists, built
//!   once at load, with a uniform grid for [`NavGraph::locate`].
//! - **The search** ([`search`]): [`PathSearch`], A* over polygons on a pool sized at load, with a
//!   limit on the nodes one search may spend.
//! - **The funnel** ([`funnel`]): a corridor of polygons to the corners a character walks.
//!
//! # Does not own
//!
//! - The format: `sapphire_navmesh`. Baking: `sapphire_pipeline`'s `nav` module.
//! - Following a path, when to search again, and keeping characters apart.
//! - Height under a character: the graph has polygons alone, no detail mesh, so a position's height
//!   here is good for telling floors apart. Feet stay on the ground by the controller's own casts.
//!
//! # Determinism
//!
//! Scalars and `Vec3`, in `+ - * /` and `sqrt`, which every architecture rounds alike. Searches break
//! ties by polygon ID, and nothing depends on an address or a hash's order. Which polygons a location
//! prefers is written out too: the lowest ID among equals.

pub mod funnel;
pub mod graph;
pub mod search;

pub use funnel::{FunnelEnd, straighten};
pub use graph::{Location, NavGraph, Reach};
pub use search::{PathSearch, Route, RouteEnd, SearchStats};
