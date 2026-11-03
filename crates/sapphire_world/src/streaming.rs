//! Activating a zone's cells by distance, within a budget (spec §5.21, M3 W2).
//!
//! A [`Streamer`] holds a zone and a focus, the point presentation looks from. Each frame it asks for the
//! meshes of the cells near the focus, turns the ones that have landed into entities of whatever hosts it
//! ([`CellHost`]), and takes apart what the focus has left, within [`StreamBudget`]'s N entities or M µs.
//! The [backdrop](sapphire_zone::Place::Backdrop) stays for as long as the zone does.
//!
//! # Three radii
//!
//! By a place's bounds' distance from the focus in XZ: within [`StreamSettings::activate`] its entities
//! exist; within [`StreamSettings::load`] its meshes are on their way, so walking up to it finds them
//! ready; past [`StreamSettings::release`] its entities go and its handles with them. Between them a
//! place stays as it is, which is what stops a walk along a radius flapping.
//!
//! # Presentation only
//!
//! **The simulation streams nothing** (m3-plan §3). Nothing here runs in a fixed step, and no simulation
//! crate can link this one (`[simulation]` in `xtask/layers.toml`). A cell's entities draw and that's
//! all: the sandbox's `replays` test runs a fight with streaming off, on, and on another budget, and the
//! checksums hold.
//!
//! # What going inactive gives back
//!
//! The streamer's handles, which is all it holds. A released mesh stays cached in the server until a
//! budget needs the room or its pack is unmounted, as anything unused does. What the renderer holds for
//! it goes when the server unloads it, which the app sees to (`GpuAssets`, M3 W3).

use std::time::Duration;

use sapphire_assets::mesh::Mesh;
use sapphire_assets::{AssetId, AssetServer, Handle, LoadPriority, LoadState, Upload, UploadError};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_time::Stopwatch;
use sapphire_zone::{ArchivedCookedLight, ArchivedPlaceContents, ArchivedZonePresentation, Bounds, Zone};

const LOG_TARGET: &str = "sapphire_world::streaming";

/// How much activation may do in a frame: whichever runs out first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamBudget {
    /// Entities placed or removed.
    pub entities: u32,
    /// Time in [`Streamer::update`], in microseconds. This is also what [`StreamStats`] is read against:
    /// m3-plan §3.2's row for streaming activation.
    pub micros: u32,
}

impl StreamBudget {
    /// §3.2's row on Desktop-Low.
    pub const DESKTOP: Self = Self { entities: 64, micros: 500 };
    /// §3.2's row on Mobile-Low.
    pub const MOBILE: Self = Self { entities: 32, micros: 1_000 };
}

/// How a zone streams: three radii in metres, in rising order (see the [module docs](self)), and a budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamSettings {
    pub activate: f32,
    pub load: f32,
    pub release: f32,
    pub budget: StreamBudget,
}

impl StreamSettings {
    /// Desktop's: past the fog's first distance at a street's scale, and about a hundred 64 m cells.
    pub const DESKTOP: Self =
        Self { activate: 256.0, load: 320.0, release: 384.0, budget: StreamBudget::DESKTOP };
    /// A phone's: a quarter of the cells, for a quarter of §3.4's draws.
    pub const MOBILE: Self =
        Self { activate: 128.0, load: 176.0, release: 224.0, budget: StreamBudget::MOBILE };
}

/// What hosts a zone's entities: the streamer says what should exist, in the zone's space, and this makes
/// it so (the stage does, with `RenderMesh` and `RenderLight`).
pub trait CellHost {
    /// One thing placed, to be given back to [`Self::remove`].
    type Placed;

    /// Places `mesh`, which is ready in the asset server, at `transform`.
    fn place_mesh(&mut self, mesh: AssetId, transform: Mat4) -> Self::Placed;

    /// Places `light`.
    fn place_light(&mut self, light: &ArchivedCookedLight) -> Self::Placed;

    /// Removes what [`Self::place_mesh`] or [`Self::place_light`] gave.
    fn remove(&mut self, placed: Self::Placed);
}

/// What the streamer did and where it stands (R9), read against [`StreamBudget`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Cells, and the backdrop if there is one.
    pub places: u32,
    /// Places with all their entities.
    pub active: u32,
    /// Places whose meshes are on their way.
    pub loading: u32,
    /// Places with their meshes held and not all their entities.
    pub loaded: u32,
    /// Places that couldn't load, which try again once the focus has left and come back.
    pub failed: u32,
    /// Places with work still to do.
    pub pending: u32,
    /// Entities that exist.
    pub entities: u32,
    /// This frame's work.
    pub placed: u32,
    pub removed: u32,
    pub time: Duration,
    /// The slowest frame so far, with frames that placed nothing counted.
    pub peak_time: Duration,
    /// Frames so far that took longer than [`StreamBudget::micros`].
    pub over_budget_frames: u64,
    /// Entities placed and removed since the streamer started.
    pub total_placed: u64,
    pub total_removed: u64,
}

/// How far a place's meshes have got.
enum Meshes {
    /// Not asked for, or let go of.
    None,
    Loading(Vec<Handle<Mesh>>),
    Loaded(Vec<Handle<Mesh>>),
    /// Stays so, and takes no time, until the focus has been past [`StreamSettings::release`].
    Failed,
}

/// What a place's distance from the focus asks of it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Want {
    /// Nothing: past the release radius.
    Nothing,
    /// Nothing changed: between the radii.
    Keep,
    /// Its meshes on their way.
    Loaded,
    /// All its entities.
    Active,
}

/// A cell, or the backdrop.
struct StreamPlace<P> {
    /// The contents' index: a cell's, or [`BACKDROP`].
    index: u32,
    /// Its bounds' least and greatest corners, in XZ.
    least: [f32; 2],
    most: [f32; 2],
    /// What it needs loaded before anything of it is placed. Its whole mesh, then each instance's.
    mesh_ids: Box<[AssetId]>,
    /// What it places: its whole mesh, its instances, then its lights.
    items: u32,
    distance: f32,
    want: Want,
    meshes: Meshes,
    /// What exists: its first `placed.len()` items, so taking some away and adding them again needs no
    /// other state.
    placed: Vec<P>,
}

/// Which [`StreamPlace`] is the backdrop.
const BACKDROP: u32 = u32::MAX;

impl<P> StreamPlace<P> {
    fn new(index: u32, contents: &ArchivedPlaceContents, zone: &ArchivedZonePresentation) -> Self {
        let Bounds { min, max } = contents.bounds();
        let instances = contents.instances();
        let whole = contents.mesh.as_ref().copied();
        let of_instances = instances.clone().map(|index| zone.instances[index].mesh);
        let mesh_ids: Box<[AssetId]> = whole.into_iter().chain(of_instances).collect();
        let items = whole.is_some() as usize + instances.len() + contents.lights().len();
        Self {
            index,
            least: [min[0], min[2]],
            most: [max[0], max[2]],
            mesh_ids,
            items: items as u32,
            distance: f32::INFINITY,
            want: Want::Nothing,
            meshes: Meshes::None,
            placed: Vec::new(),
        }
    }

    /// Its contents in the zone.
    fn contents<'a>(&self, zone: &'a ArchivedZonePresentation) -> &'a ArchivedPlaceContents {
        if self.index == BACKDROP {
            zone.backdrop.as_ref().expect("a backdrop place is only made from a zone's backdrop")
        } else {
            &zone.cells[self.index as usize].contents
        }
    }

    /// Whether what the focus asks of it is done, or can't be until something changes.
    fn settled(&self) -> bool {
        let complete = self.placed.len() == self.items as usize;
        match (self.want, &self.meshes) {
            (Want::Nothing, Meshes::None | Meshes::Failed) => self.placed.is_empty(),
            (Want::Nothing, _) => false,
            (Want::Active, Meshes::Failed) => true,
            (Want::Active, _) => complete,
            (Want::Loaded, Meshes::None | Meshes::Loading(_)) => false,
            (Want::Loaded, Meshes::Loaded(_) | Meshes::Failed) => true,
            (Want::Keep, Meshes::Loading(_)) => false,
            (Want::Keep, _) => true,
        }
    }
}

/// A zone's cells, activating and going inactive around a focus. See the [module docs](self).
pub struct Streamer<P> {
    zone: Handle<Zone>,
    settings: StreamSettings,
    /// The version of the zone `places` was made from.
    built: Option<u32>,
    places: Vec<StreamPlace<P>>,
    /// Places with work to do, nearest the focus first, as of the last rescan.
    pending: Vec<u32>,
    /// Where the focus was at the last rescan.
    scanned: Option<[f32; 2]>,
    /// The next update places everything it can, after a load screen ([`Self::preload`]).
    unbudgeted: bool,
    stats: StreamStats,
}

impl<P> Streamer<P> {
    /// A streamer over `zone`, with nothing placed. The zone's cells load as soon as it has.
    #[must_use]
    pub fn new(zone: Handle<Zone>, settings: StreamSettings) -> Self {
        debug_assert!(settings.activate <= settings.load && settings.load < settings.release);
        Self {
            zone,
            settings,
            built: None,
            places: Vec::new(),
            pending: Vec::new(),
            scanned: None,
            unbudgeted: false,
            stats: StreamStats::default(),
        }
    }

    /// The zone.
    #[must_use]
    pub fn zone(&self) -> &Handle<Zone> {
        &self.zone
    }

    /// What it did last frame and where it stands.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        self.stats
    }

    /// What exists, in no order.
    pub fn placed_mut(&mut self) -> impl Iterator<Item = &mut P> {
        self.places.iter_mut().flat_map(|place| place.placed.iter_mut())
    }

    /// The load screen's half: loads until the meshes of everything within the activation radius of
    /// `focus` (in the zone's space) are ready, sending what lands through `upload`, so that the next
    /// [`Self::update`] places all of them in one frame. Call with the zone ready. A mesh that doesn't
    /// load shows in [`StreamStats::failed`] with a warning, like any other.
    pub fn preload(
        &mut self,
        focus: [f32; 3],
        server: &mut AssetServer,
        mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>,
    ) {
        profiling::scope!("Streamer::preload");
        if !self.settled_on_zone(server) {
            return;
        }
        self.rescan([focus[0], focus[2]]);
        for place in &mut self.places {
            if place.want != Want::Active {
                continue;
            }
            if matches!(place.meshes, Meshes::None) {
                ask(place, server, LoadPriority::Critical);
            }
            if let Meshes::Loading(handles) = &place.meshes {
                let finished: Result<(), _> =
                    handles.iter().try_for_each(|handle| server.finish(handle, &mut upload));
                match finished {
                    Ok(()) => take_loaded(place),
                    Err(error) => {
                        tracing::warn!(target: LOG_TARGET, place = place.index, %error, "a place's meshes didn't load");
                        place.meshes = Meshes::Failed;
                    }
                }
            }
        }
        self.unbudgeted = true;
    }

    /// Loads until what the streamer holds is ready again, sending it through `upload`: a new renderer's
    /// load screen, as what's placed draws meshes it doesn't have yet. What doesn't load is warned of,
    /// and stays placed with whatever the app gives it.
    pub fn finish_held(
        &mut self,
        server: &mut AssetServer,
        mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>,
    ) {
        profiling::scope!("Streamer::finish_held");
        for place in &self.places {
            if let Meshes::Loaded(handles) = &place.meshes {
                for handle in handles {
                    if let Err(error) = server.finish(handle, &mut upload) {
                        tracing::warn!(target: LOG_TARGET, place = place.index, %error, "a place's mesh didn't reload");
                    }
                }
            }
        }
    }

    /// Does this frame's work for a focus at `focus`, in the zone's space: anything from nothing to the
    /// budget's worth, and always one thing placed and one removed if any is waiting, so a tiny budget
    /// is slow rather than stuck. Allocates only for places that change.
    pub fn update<H: CellHost<Placed = P>>(
        &mut self,
        focus: [f32; 3],
        server: &mut AssetServer,
        host: &mut H,
    ) {
        profiling::scope!("Streamer::update");
        let stopwatch = Stopwatch::start();
        self.stats.placed = 0;
        self.stats.removed = 0;
        if self.settled_on_zone_hosted(server, host) {
            let focus = [focus[0], focus[2]];
            let moved = self.scanned.is_none_or(|scanned| {
                let (x, z) = (focus[0] - scanned[0], focus[1] - scanned[1]);
                x * x + z * z > self.settings.activate * self.settings.activate / 1024.0
            });
            if moved {
                self.rescan(focus);
            }
            self.work(&stopwatch, server, host);
        }
        self.publish(stopwatch.elapsed());
    }

    /// Takes everything down at once and lets go of what it holds, the zone's handle aside: a zone
    /// leaving (a wipe hides this, M3 W4). A streamer can go on after this, from nothing.
    pub fn clear<H: CellHost<Placed = P>>(&mut self, host: &mut H) {
        profiling::scope!("Streamer::clear");
        for place in &mut self.places {
            for placed in place.placed.drain(..) {
                host.remove(placed);
                self.stats.total_removed += 1;
            }
            place.meshes = Meshes::None;
        }
        self.places.clear();
        self.pending.clear();
        self.built = None;
        self.scanned = None;
        self.stats.entities = 0;
    }

    /// Takes everything down and gives up the zone's handle, which the caller may want to last no longer
    /// than its pack.
    pub fn close<H: CellHost<Placed = P>>(mut self, host: &mut H) {
        self.clear(host);
    }

    /// Whether the zone can be read now, with `places` made from its version, for code that has no host
    /// yet as nothing can have been placed.
    fn settled_on_zone(&mut self, server: &AssetServer) -> bool {
        let Some(generation) = ready_generation(&self.zone, server) else {
            return false;
        };
        if self.built != Some(generation) {
            debug_assert!(self.places.iter().all(|place| place.placed.is_empty()));
            self.build(generation, server);
        }
        true
    }

    /// Whether the zone can be read now, with `places` made from its version: a new version takes down
    /// whatever the old one placed, as its cells' indices and items mean something else now.
    fn settled_on_zone_hosted<H: CellHost<Placed = P>>(
        &mut self,
        server: &AssetServer,
        host: &mut H,
    ) -> bool {
        let Some(generation) = ready_generation(&self.zone, server) else {
            return false;
        };
        if self.built != Some(generation) {
            self.clear(host);
            self.build(generation, server);
        }
        true
    }

    fn build(&mut self, generation: u32, server: &AssetServer) {
        let zone = &server.get(&self.zone).expect("the zone is ready").presentation;
        let backdrop = zone.backdrop.as_ref().map(|contents| StreamPlace::new(BACKDROP, contents, zone));
        let cells = zone
            .cells
            .iter()
            .enumerate()
            .map(|(index, cell)| StreamPlace::new(index as u32, &cell.contents, zone));
        self.places = backdrop.into_iter().chain(cells).collect();
        self.pending = Vec::with_capacity(self.places.len());
        self.built = Some(generation);
        self.scanned = None;
    }

    /// Goes by a focus at `focus`: each place's distance and what that asks of it, and the work that
    /// makes, nearest first.
    fn rescan(&mut self, focus: [f32; 2]) {
        profiling::scope!("Streamer::rescan");
        self.scanned = Some(focus);
        let StreamSettings { activate, load, release, .. } = self.settings;
        for place in &mut self.places {
            place.distance = if place.index == BACKDROP {
                0.0
            } else {
                let past = |axis: usize| {
                    (place.least[axis] - focus[axis]).max(focus[axis] - place.most[axis]).max(0.0)
                };
                let (x, z) = (past(0), past(1));
                (x * x + z * z).sqrt()
            };
            place.want = match place.distance {
                distance if distance <= activate => Want::Active,
                distance if distance <= load => Want::Loaded,
                distance if distance <= release => Want::Keep,
                _ => Want::Nothing,
            };
            // Coming back is what tries a failed place again.
            if place.want == Want::Nothing && matches!(place.meshes, Meshes::Failed) {
                place.meshes = Meshes::None;
            }
        }
        self.pending.clear();
        self.pending.extend(
            self.places
                .iter()
                .enumerate()
                .filter(|(_, place)| !place.settled())
                .map(|(index, _)| index as u32),
        );
        let places = &self.places;
        self.pending
            .sort_unstable_by(|&a, &b| places[a as usize].distance.total_cmp(&places[b as usize].distance));
    }

    /// Goes through the waiting places, in the order that hides the most: what's asked for, what's
    /// landed, what's near, then what's far.
    fn work<H: CellHost<Placed = P>>(
        &mut self,
        stopwatch: &Stopwatch,
        server: &mut AssetServer,
        host: &mut H,
    ) {
        let budget = self.settings.budget;
        let unbudgeted = std::mem::take(&mut self.unbudgeted);
        let limit = Duration::from_micros(u64::from(budget.micros));
        let spent = |count: u32| !unbudgeted && (count >= budget.entities || stopwatch.elapsed() >= limit);

        // Asking costs a little: a slot and a read queued, for each mesh. Always one place, as with entities.
        let mut asked = 0;
        for &index in &self.pending {
            let place = &mut self.places[index as usize];
            if place.want >= Want::Loaded && matches!(place.meshes, Meshes::None) {
                let priority =
                    if place.want == Want::Active { LoadPriority::High } else { LoadPriority::Normal };
                ask(place, server, priority);
                asked += 1;
                if !unbudgeted && asked >= 1 && stopwatch.elapsed() >= limit {
                    break;
                }
            }
        }
        for &index in &self.pending {
            let place = &mut self.places[index as usize];
            if let Meshes::Loading(handles) = &place.meshes {
                let mut ready = true;
                let mut failure = None;
                for handle in handles {
                    match server.state(handle) {
                        LoadState::Ready => {}
                        LoadState::Failed(error) => {
                            failure = Some(error);
                            break;
                        }
                        _ => ready = false,
                    }
                }
                if let Some(error) = failure {
                    tracing::warn!(target: LOG_TARGET, place = place.index, %error, "a place's meshes didn't load");
                    place.meshes = Meshes::Failed;
                } else if ready {
                    take_loaded(place);
                }
            }
        }

        let zone = &server.get(&self.zone).expect("the zone is ready").presentation;
        let mut count = 0;
        'placing: for &index in &self.pending {
            let place = &mut self.places[index as usize];
            if place.want != Want::Active || !matches!(place.meshes, Meshes::Loaded(_)) {
                continue;
            }
            let contents = place.contents(zone);
            while place.placed.len() < place.items as usize {
                if count > 0 && spent(count) {
                    break 'placing;
                }
                place.placed.push(place_item(place.placed.len(), contents, zone, host));
                count += 1;
            }
        }
        self.stats.placed = count;
        self.stats.total_placed += u64::from(count);

        let mut removed = 0;
        'removing: for &index in self.pending.iter().rev() {
            let place = &mut self.places[index as usize];
            if place.want != Want::Nothing {
                continue;
            }
            while !place.placed.is_empty() {
                if removed > 0 && spent(count + removed) {
                    break 'removing;
                }
                host.remove(place.placed.pop().expect("not empty"));
                removed += 1;
            }
            place.meshes = Meshes::None;
        }
        self.stats.removed = removed;
        self.stats.total_removed += u64::from(removed);

        let places = &self.places;
        self.pending.retain(|&index| !places[index as usize].settled());
    }

    fn publish(&mut self, time: Duration) {
        let stats = &mut self.stats;
        stats.places = self.places.len() as u32;
        (stats.active, stats.loading, stats.loaded, stats.failed) = (0, 0, 0, 0);
        stats.entities = 0;
        for place in &self.places {
            stats.entities += place.placed.len() as u32;
            match &place.meshes {
                Meshes::None => {}
                Meshes::Loading(_) => stats.loading += 1,
                Meshes::Failed => stats.failed += 1,
                Meshes::Loaded(_) if place.placed.len() == place.items as usize => stats.active += 1,
                Meshes::Loaded(_) => stats.loaded += 1,
            }
        }
        stats.pending = self.pending.len() as u32;
        stats.time = time;
        stats.peak_time = stats.peak_time.max(time);
        if time > Duration::from_micros(u64::from(self.settings.budget.micros)) {
            stats.over_budget_frames += 1;
        }
    }
}

/// The zone's version, if it can be read.
fn ready_generation(zone: &Handle<Zone>, server: &AssetServer) -> Option<u32> {
    (server.state(zone) == LoadState::Ready).then(|| server.generation(zone))
}

/// Asks for a place's meshes. A pack that's gone is a failed place like any other.
fn ask<P>(place: &mut StreamPlace<P>, server: &mut AssetServer, priority: LoadPriority) {
    let handles: Result<Vec<Handle<Mesh>>, _> =
        place.mesh_ids.iter().map(|&id| server.load_with(id, priority)).collect();
    place.meshes = match handles {
        Ok(handles) => Meshes::Loading(handles),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, place = place.index, %error, "a place's meshes aren't in any pack");
            Meshes::Failed
        }
    };
}

/// Moves a place whose meshes are ready on.
fn take_loaded<P>(place: &mut StreamPlace<P>) {
    if let Meshes::Loading(handles) = std::mem::replace(&mut place.meshes, Meshes::None) {
        place.meshes = Meshes::Loaded(handles);
    }
}

/// Places item `item` of `contents`: its whole mesh, its instances, then its lights.
fn place_item<H: CellHost>(
    mut item: usize,
    contents: &ArchivedPlaceContents,
    zone: &ArchivedZonePresentation,
    host: &mut H,
) -> H::Placed {
    if let Some(&mesh) = contents.mesh.as_ref() {
        if item == 0 {
            // Its vertices are in the zone's space.
            return host.place_mesh(mesh, Mat4::IDENTITY);
        }
        item -= 1;
    }
    let instances = contents.instances();
    if item < instances.len() {
        let instance = &zone.instances[instances.start + item];
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::from(instance.scale()),
            Quat::from_array(instance.rotation()),
            Vec3::from(instance.position()),
        );
        return host.place_mesh(instance.mesh, transform);
    }
    host.place_light(&zone.lights[contents.lights().start + item - instances.len()])
}

#[cfg(test)]
mod tests;
