//! The streamer on a synthetic zone, with a host that only keeps count: no GPU, and an uploader that
//! accepts everything.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::{AssetBudgets, AssetKind, MemoryPackSource, MountState};
use sapphire_synthetic_zone::SyntheticZone;

use super::*;

const SMALL: SyntheticZone = SyntheticZone::SMALL;
const PACK: &str = "zone_synthetic";
/// Far enough from the small zone that nothing's within any radius.
const FAR: [f32; 3] = [100_000.0, 0.0, 100_000.0];

/// A host that remembers what's placed.
#[derive(Default)]
struct Ledger {
    next: u32,
    meshes: Vec<(u32, AssetId, Mat4)>,
    lights: Vec<u32>,
    removed: u64,
}

impl CellHost for Ledger {
    type Placed = u32;

    fn place_mesh(&mut self, mesh: AssetId, transform: Mat4) -> u32 {
        self.next += 1;
        self.meshes.push((self.next, mesh, transform));
        self.next
    }

    fn place_light(&mut self, _: &ArchivedCookedLight) -> u32 {
        self.next += 1;
        self.lights.push(self.next);
        self.next
    }

    fn remove(&mut self, placed: u32) {
        let before = self.live();
        self.meshes.retain(|&(which, ..)| which != placed);
        self.lights.retain(|&which| which != placed);
        assert_eq!(self.live() + 1, before, "{placed} was removed twice, or never placed");
        self.removed += 1;
    }
}

impl Ledger {
    fn live(&self) -> usize {
        self.meshes.len() + self.lights.len()
    }

    fn has(&self, mesh: AssetId) -> bool {
        self.meshes.iter().any(|&(_, placed, _)| placed == mesh)
    }
}

fn accept(_: Upload<'_>) -> Result<(), UploadError> {
    Ok(())
}

fn unlimited() -> AssetBudgets {
    AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] }
}

fn server_over(zone: SyntheticZone) -> (AssetServer, Arc<MemoryPackSource>) {
    let source = Arc::new(MemoryPackSource::new());
    source.insert(PACK, zone.pack(Vec::new()).unwrap());
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount_now(PACK).unwrap();
    (server, source)
}

/// 64 m around the focus, with each radius well apart, on a generous budget.
fn settings() -> StreamSettings {
    StreamSettings {
        activate: 64.0,
        load: 96.0,
        release: 128.0,
        budget: StreamBudget { entities: 1_000, micros: 1_000_000 },
    }
}

fn streamer(server: &mut AssetServer, settings: StreamSettings) -> Streamer<u32> {
    let zone: Handle<Zone> = server.load(SyntheticZone::id()).unwrap();
    server.finish(&zone, accept).unwrap();
    Streamer::new(zone, settings)
}

/// A frame of the app: the server's, then the streamer's.
fn frame(streamer: &mut Streamer<u32>, focus: [f32; 3], server: &mut AssetServer, host: &mut Ledger) {
    server.update();
    server.upload(accept);
    streamer.update(focus, server, host);
}

/// Frames until nothing's left to do.
fn settle(streamer: &mut Streamer<u32>, focus: [f32; 3], server: &mut AssetServer, host: &mut Ledger) {
    let start = Stopwatch::start();
    frame(streamer, focus, server, host);
    while streamer.stats().pending > 0 || streamer.stats().loading > 0 {
        server.wait(Duration::from_millis(1));
        frame(streamer, focus, server, host);
        assert!(start.elapsed() < Duration::from_secs(60), "it stalled: {:?}", streamer.stats());
    }
}

/// How many cells of `zone` lie within `radius` of a focus at `(x, z)`, by their squares.
fn cells_within(zone: SyntheticZone, (x, z): (f32, f32), radius: f32) -> u32 {
    let mut count = 0;
    for cell_x in 0..zone.side {
        for cell_z in 0..zone.side {
            let past = |cell: i32, focus: f32| {
                let start = cell as f32 * zone.cell_size;
                (start - focus).max(focus - (start + zone.cell_size)).max(0.0)
            };
            if past(cell_x, x).hypot(past(cell_z, z)) <= radius {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn what_exists_is_what_the_focus_is_near() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    let focus = [16.0, 0.0, 16.0];
    settle(&mut streamer, focus, &mut server, &mut host);

    let within = cells_within(SMALL, (16.0, 16.0), 64.0);
    assert!(within > 1 && within < 64, "{within}");
    let per_cell = 1 + SMALL.instances + SMALL.lights;
    let stats = streamer.stats();
    assert_eq!(stats.places, 65);
    assert_eq!(stats.active, within + 1, "and the backdrop");
    assert_eq!(stats.entities, within * per_cell + 1);
    assert_eq!(host.live() as u32, stats.entities);
    assert_eq!(host.lights.len() as u32, within * SMALL.lights);
    assert!(host.has(SyntheticZone::backdrop_mesh()));
    assert!(host.has(SyntheticZone::cell_mesh(0, 0)));
    assert!(!host.has(SyntheticZone::cell_mesh(7, 7)));
    // Only instances move: a cell's merged mesh is in the zone's space.
    let instances: Vec<_> =
        host.meshes.iter().filter(|&&(_, mesh, _)| mesh == SyntheticZone::instance_mesh()).collect();
    assert_eq!(instances.len() as u32, within * SMALL.instances);
    assert!(instances.iter().all(|&&(_, _, transform)| transform != Mat4::IDENTITY));
    assert!(
        host.meshes
            .iter()
            .filter(|&&(_, mesh, _)| mesh != SyntheticZone::instance_mesh())
            .all(|&(_, _, transform)| transform == Mat4::IDENTITY)
    );
    // An instance goes where the zone says, whole: position and scale.
    let (_, _, first) = **instances.first().unwrap();
    let (scale, _, position) = first.to_scale_rotation_translation();
    assert_eq!(scale, Vec3::splat(2.0));
    assert_eq!(position.y, 0.0);
    assert!(position.x > 0.0 && position.z > 0.0);
}

#[test]
fn the_backdrop_stays_wherever_the_focus_goes() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    settle(&mut streamer, FAR, &mut server, &mut host);
    assert_eq!(streamer.stats().active, 1);
    assert_eq!(host.live(), 1);
    assert!(host.has(SyntheticZone::backdrop_mesh()));
}

#[test]
fn no_frame_goes_past_the_entity_budget_and_the_nearest_comes_first() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let budget = StreamBudget { entities: 5, micros: 1_000_000 };
    let mut streamer = streamer(&mut server, StreamSettings { budget, ..settings() });
    let focus = [16.0, 0.0, 16.0];
    // Load everything first, so the order is the placing's and not the loads': the streamer places what
    // has landed, and which of a frame's reads lands first is the reader threads', so under load a far
    // cell's could beat a near one's. Held here, each is ready the frame the streamer asks for it.
    let cells = (0..SMALL.side).flat_map(|x| (0..SMALL.side).map(move |z| SyntheticZone::cell_mesh(x, z)));
    let meshes = [SyntheticZone::backdrop_mesh(), SyntheticZone::instance_mesh()].into_iter().chain(cells);
    let _loaded: Vec<Handle<Mesh>> = meshes
        .map(|id| {
            let mesh = server.load(id).unwrap();
            server.finish(&mesh, accept).unwrap();
            mesh
        })
        .collect();
    let mut first_seen: Vec<AssetId> = Vec::new();
    let mut seen = BTreeSet::new();
    let start = Stopwatch::start();
    loop {
        server.update();
        server.upload(accept);
        server.wait(Duration::from_millis(1));
        streamer.update(focus, &mut server, &mut host);
        let stats = streamer.stats();
        assert!(stats.placed <= budget.entities, "{stats:?}");
        for &(_, mesh, _) in &host.meshes {
            if mesh != SyntheticZone::instance_mesh() && seen.insert(mesh) {
                first_seen.push(mesh);
            }
        }
        if stats.pending == 0 && stats.loading == 0 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "{stats:?}");
    }
    let cell_at = |x, z| first_seen.iter().position(|&mesh| mesh == SyntheticZone::cell_mesh(x, z)).unwrap();
    assert_eq!(first_seen[0], SyntheticZone::backdrop_mesh());
    assert!(cell_at(0, 0) < cell_at(1, 1), "{first_seen:?}");
}

#[test]
fn a_budget_of_one_is_slow_and_gets_there() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let budget = StreamBudget { entities: 1, micros: 0 };
    let mut streamer = streamer(&mut server, StreamSettings { budget, ..settings() });
    let focus = [16.0, 0.0, 16.0];
    settle(&mut streamer, focus, &mut server, &mut host);
    let within = cells_within(SMALL, (16.0, 16.0), 64.0);
    assert_eq!(host.live() as u32, within * (1 + SMALL.instances + SMALL.lights) + 1);
    // And taking it all down, a thing a frame, gets there as well.
    settle(&mut streamer, FAR, &mut server, &mut host);
    assert_eq!(host.live(), 1);
    assert_eq!(streamer.stats().total_placed - streamer.stats().total_removed, 1);
}

#[test]
fn a_walk_across_leaves_nothing_behind_and_lets_go_of_what_it_passed() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let budget = StreamBudget { entities: 8, micros: 1_000_000 };
    let mut streamer = streamer(&mut server, StreamSettings { budget, ..settings() });
    let width = SMALL.cell_size * SMALL.side as f32;
    let mut position = -200.0;
    while position < width + 200.0 {
        frame(&mut streamer, [position, 0.0, width / 2.0], &mut server, &mut host);
        assert!(streamer.stats().placed <= 8);
        position += 0.5;
    }
    settle(&mut streamer, FAR, &mut server, &mut host);
    assert_eq!(host.live(), 1, "only the backdrop");
    let stats = streamer.stats();
    assert_eq!(stats.total_placed - stats.total_removed, 1);
    assert_eq!((stats.active, stats.loading, stats.loaded, stats.failed), (1, 0, 0, 0));

    // Its handles have gone: everything it passed is unused, a cache, save the backdrop's mesh and its
    // material. What it let go of on the way is still landing, and only then can it be dropped.
    let start = Stopwatch::start();
    while server.stats().loading + server.stats().waiting > 0 {
        server.wait(Duration::from_millis(1));
        server.update();
        server.upload(accept);
        assert!(start.elapsed() < Duration::from_secs(60), "it stalled: {:?}", server.stats());
    }
    let cached = server.stats();
    assert!(cached.unused > 0);
    server.unload_unused();
    server.update();
    let remaining = server.stats();
    assert_eq!(remaining.assets, 3, "the zone, the backdrop and its material: {remaining:?}");
}

#[test]
fn walking_up_and_down_a_radius_doesnt_flap() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
    // Walk away till (7, 0) is just past the radius, then back and forth over the line.
    let start = 7.0 * SMALL.cell_size;
    let line = start - settings().activate;
    settle(&mut streamer, [line, 0.0, 16.0], &mut server, &mut host);
    let (placed, removed) = (streamer.stats().total_placed, streamer.stats().total_removed);
    assert!(host.has(SyntheticZone::cell_mesh(7, 0)));
    for step in 0..200 {
        let across = if step % 2 == 0 { 1.0 } else { -1.0 };
        frame(&mut streamer, [line + across, 0.0, 16.0], &mut server, &mut host);
    }
    assert_eq!(streamer.stats().total_placed, placed);
    assert_eq!(streamer.stats().total_removed, removed);
}

#[test]
fn a_place_that_cant_load_is_counted_and_comes_back_with_its_pack() {
    // A pack with the zone and only some of its meshes: the rest live in a pack that isn't there yet.
    let source = Arc::new(MemoryPackSource::new());
    let zone = SyntheticZone { side: 1, cell_size: 32.0, instances: 0, lights: 0, quads: 1, texture: 0 };
    let whole = zone.pack(Vec::new()).unwrap();
    source.insert(PACK, whole.clone());
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount_now(PACK).unwrap();
    let handle: Handle<Zone> = server.load(SyntheticZone::id()).unwrap();
    server.finish(&handle, accept).unwrap();
    let mut streamer = Streamer::new(handle.clone(), settings());
    let mut host = Ledger::default();
    settle(&mut streamer, [1.0, 0.0, 1.0], &mut server, &mut host);
    assert_eq!(streamer.stats().active, 2);

    // Pack gone, with the zone's handle holding it: what the streamer holds stays, as a handle does.
    let report = server.unmount(PACK).unwrap();
    assert_eq!(report.unloaded, 0);
    frame(&mut streamer, [1.0, 0.0, 1.0], &mut server, &mut host);
    assert_eq!(streamer.stats().active, 2);

    // Past the release radius its handles go, and with no pack they unload. Coming back, nothing can
    // load the cell, and that's a counted failure rather than a panic. The backdrop keeps what it has.
    settle(&mut streamer, FAR, &mut server, &mut host);
    frame(&mut streamer, FAR, &mut server, &mut host);
    assert_eq!(host.live(), 1);
    server.update();
    settle(&mut streamer, [1.0, 0.0, 1.0], &mut server, &mut host);
    let stats = streamer.stats();
    assert_eq!((stats.failed, stats.active, host.live()), (1, 1, 1), "{stats:?}");

    // It tries once, not every frame: nothing's pending while it's near.
    assert_eq!(stats.pending, 0);
    // And leaving and coming back with the pack mounted heals it.
    settle(&mut streamer, FAR, &mut server, &mut host);
    server.mount_now(PACK).unwrap();
    settle(&mut streamer, [1.0, 0.0, 1.0], &mut server, &mut host);
    assert_eq!((streamer.stats().failed, streamer.stats().active, host.live()), (0, 2, 2));
}

#[test]
fn closing_with_cells_active_removes_them_and_the_pack_can_go() {
    let (mut server, source) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
    assert!(host.live() > 1);

    streamer.close(&mut host);
    assert_eq!(host.live(), 0);
    server.update();
    let report = server.unmount(PACK).unwrap();
    assert_eq!(report.held, 0, "{report:?}");
    assert_eq!(server.stats().assets, 0);
    assert!(matches!(server.mount_state(PACK), MountState::Unmounted));
    drop(source);
}

#[test]
fn a_new_version_of_the_zone_starts_over_from_its_cells() {
    let (mut server, source) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
    assert!(host.has(SyntheticZone::cell_mesh(0, 0)));

    // The same IDs, cut differently: more instances in fewer cells, so yesterday's indices are wrong.
    let next = SyntheticZone { side: 2, instances: 5, lights: 0, ..SMALL };
    source.insert(PACK, next.pack(Vec::new()).unwrap());
    server.remount(PACK).unwrap();
    settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
    let start = Stopwatch::start();
    while streamer.stats().places != 5 {
        server.wait(Duration::from_millis(1));
        settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
        assert!(start.elapsed() < Duration::from_secs(30), "{:?}", streamer.stats());
    }
    assert_eq!(host.lights.len(), 0);
    assert_eq!(host.live(), 1 + 4 + 4 * 5);
}

#[test]
fn a_preload_makes_the_first_update_whole_whatever_the_budget() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let budget = StreamBudget { entities: 1, micros: 0 };
    let mut streamer = streamer(&mut server, StreamSettings { budget, ..settings() });
    let focus = [16.0, 0.0, 16.0];
    streamer.preload(focus, &mut server, accept);
    streamer.update(focus, &mut server, &mut host);
    let within = cells_within(SMALL, (16.0, 16.0), 64.0);
    assert_eq!(host.live() as u32, within * (1 + SMALL.instances + SMALL.lights) + 1);
    // The next frame's back on the budget.
    settle(&mut streamer, FAR, &mut server, &mut host);
    streamer.update([16.0, 0.0, 16.0], &mut server, &mut host);
    assert!(streamer.stats().placed <= 1);
}

#[test]
fn an_idle_streamer_does_nothing_and_allocates_nothing_it_can_see() {
    let (mut server, _) = server_over(SMALL);
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    let focus = [16.0, 0.0, 16.0];
    settle(&mut streamer, focus, &mut server, &mut host);
    let before = streamer.stats();
    for _ in 0..100 {
        frame(&mut streamer, focus, &mut server, &mut host);
    }
    let after = streamer.stats();
    assert_eq!((after.placed, after.removed, after.pending), (0, 0, 0));
    assert_eq!(after.total_placed, before.total_placed);
    assert_eq!(after.entities, before.entities);
}

#[test]
fn no_mesh_reaches_a_host_before_the_server_says_its_ready() {
    // On a real read path the meshes land frames after they're asked for.
    let (mut server, _) = server_over(SyntheticZone { quads: 16, ..SMALL });
    let mut host = Ledger::default();
    let mut streamer = streamer(&mut server, settings());
    let focus = [16.0, 0.0, 16.0];
    streamer.update(focus, &mut server, &mut host);
    assert!(streamer.stats().loading > 0, "{:?}", streamer.stats());
    assert_eq!(host.live(), 0, "asking isn't placing");
    settle(&mut streamer, focus, &mut server, &mut host);
    assert!(host.live() > 0);
}

#[test]
fn shared_dependencies_are_held_once_but_every_instance_is_placed() {
    // The merged mesh makes these input counts 7, 8, 9 and 26.
    for instances in [6, 7, 8, 25] {
        let zone = SyntheticZone { side: 1, instances, lights: 0, ..SMALL };
        let (mut server, _) = server_over(zone);
        let mut streamer = streamer(&mut server, settings());
        let mut host = Ledger::default();
        settle(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
        let cell = streamer.places.iter().find(|place| place.index == 0).unwrap();
        assert_eq!(
            cell.mesh_ids.as_ref(),
            &[SyntheticZone::cell_mesh(0, 0), SyntheticZone::instance_mesh()],
            "first occurrence order is also the order missing dependencies are diagnosed"
        );
        assert_eq!(
            host.meshes.iter().filter(|(_, mesh, _)| *mesh == SyntheticZone::instance_mesh()).count(),
            instances as usize
        );
        let before = streamer.stats();
        for _ in 0..4 {
            frame(&mut streamer, [16.0, 0.0, 16.0], &mut server, &mut host);
        }
        let after = streamer.stats();
        assert_eq!(
            (after.active, after.loaded, after.loading, after.failed, after.entities, after.pending),
            (before.active, before.loaded, before.loading, before.failed, host.live() as u32, 0)
        );
        streamer.clear(&mut host);
        let cleared = streamer.stats();
        assert_eq!(
            (
                cleared.places,
                cleared.active,
                cleared.loaded,
                cleared.loading,
                cleared.failed,
                cleared.entities,
                cleared.pending
            ),
            (0, 0, 0, 0, 0, 0, 0)
        );
        assert_eq!(host.live(), 0);
    }
}

#[test]
fn preload_after_a_zone_reload_does_not_abandon_existing_entities() {
    let (mut server, source) = server_over(SMALL);
    let mut streamer = streamer(&mut server, settings());
    let mut host = Ledger::default();
    let focus = [16.0, 0.0, 16.0];
    settle(&mut streamer, focus, &mut server, &mut host);
    let before = host.live();
    let next = SyntheticZone { side: 2, instances: 5, lights: 0, ..SMALL };
    source.insert(PACK, next.pack(Vec::new()).unwrap());
    server.remount(PACK).unwrap();
    let timer = Stopwatch::start();
    while ready_generation(streamer.zone(), &server)
        .is_none_or(|generation| streamer.built == Some(generation))
    {
        server.update();
        server.upload(accept);
        server.wait(Duration::from_millis(1));
        assert!(timer.elapsed() < Duration::from_secs(30), "the replacement did not arrive");
    }
    assert_ne!(streamer.built, ready_generation(streamer.zone(), &server));
    streamer.preload(focus, &mut server, accept);
    assert_eq!(host.live(), before, "preload cannot remove entities without a host");
    settle(&mut streamer, focus, &mut server, &mut host);
    assert_eq!(host.live(), 1 + 4 + 4 * 5);
    assert!(host.removed >= before as u64);
}

/// M3 Q3: a camera cut to the far corner of the zone, with the frames before it either asking for the cut's
/// eye ahead or not. Asked, the cut lands on its places placed and no frame is missing any; not asked, the
/// frame of the cut has its view's places still to come, which is what the ask is for.
#[test]
fn a_cut_asked_ahead_lands_on_its_places_and_one_not_asked_misses_them() {
    let (from, to) = ([16.0, 0.0, 16.0], [240.0, 0.0, 240.0]);
    let within = cells_within(SMALL, (240.0, 240.0), 64.0);
    for asked in [false, true] {
        let (mut server, _) = server_over(SMALL);
        let mut host = Ledger::default();
        let budget = StreamBudget { entities: 8, micros: 1_000_000 };
        let mut streamer = streamer(&mut server, StreamSettings { budget, ..settings() });
        settle(&mut streamer, from, &mut server, &mut host);
        assert_eq!(streamer.stats().unready, 0);
        let ahead: &[[f32; 3]] = if asked { &[to] } else { &[] };
        let start = Stopwatch::start();
        loop {
            server.update();
            server.upload(accept);
            server.wait(Duration::from_millis(1));
            streamer.update_ahead(from, ahead, &mut server, &mut host);
            let stats = streamer.stats();
            assert_eq!(stats.unready, 0, "the focus's own places stay while it asks ahead: {stats:?}");
            if stats.pending == 0 && stats.loading == 0 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "it stalled: {stats:?}");
        }
        assert_eq!(host.has(SyntheticZone::cell_mesh(7, 7)), asked);
        let missed_before = streamer.stats().unready_frames;
        // The cut.
        frame(&mut streamer, to, &mut server, &mut host);
        let stats = streamer.stats();
        if asked {
            assert_eq!((stats.unready, stats.unready_frames), (0, missed_before), "{stats:?}");
            assert!(host.has(SyntheticZone::cell_mesh(7, 7)));
        } else {
            assert!(stats.unready > 0 && stats.unready <= within, "{stats:?}");
            assert_eq!(stats.unready_frames, missed_before + 1);
            assert!(!host.has(SyntheticZone::cell_mesh(7, 7)));
        }
    }
}
