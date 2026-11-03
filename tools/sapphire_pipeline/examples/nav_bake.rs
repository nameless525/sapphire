//! Bakes the arena and the look-dev town, prints what a bake costs and writes `.snav` files for
//! `sapphire_nav`'s `nav_bench`. A cooker's numbers, so run it as the cooker builds:
//!
//! ```text
//! cargo run --profile tools -p sapphire_pipeline --example nav_bake -- <directory>
//! ```

#![allow(clippy::print_stdout)]

#[path = "../tests/nav_support/mod.rs"]
mod nav_support;

use std::path::PathBuf;

use sapphire_pipeline::nav::{BakeSettings, TileBounds, Triangle, bake, bake_tile, stitch, tile_range};
use sapphire_time::Stopwatch;

/// The tiles covering a square of `half` metres around the origin.
fn around_origin(half: f32, settings: &BakeSettings) -> TileBounds {
    let size = settings.tile_cells as f32 * settings.cell_size;
    let reach = (half / size).ceil() as i32;
    ([-reach, -reach], [reach - 1, reach - 1])
}

fn run(name: &str, triangles: &[Triangle], half: f32, settings: &BakeSettings, out: Option<&PathBuf>) {
    const ROUNDS: usize = 3;
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..ROUNDS {
        let begun = Stopwatch::start();
        let baked = bake(triangles, around_origin(half, settings), settings).expect("it bakes");
        times.push(begun.elapsed().as_secs_f64() * 1000.0);
        last = Some(baked);
    }
    times.sort_by(f64::total_cmp);
    let baked = last.expect("ran");
    let bytes = baked.mesh.to_cooked().expect("it writes");
    println!(
        "{name} ({} m tiles): {} triangles, {} tiles read, {} kept, {} following cells, {} holes lost; {} polygons, {} vertices, {} corners, {} bytes cooked; bake {:.0} ms best of {ROUNDS}, {:.0} worst",
        settings.tile_cells as f32 * settings.cell_size,
        baked.stats.triangles,
        baked.stats.tiles_read,
        baked.stats.tiles_kept,
        baked.stats.tiles_cell_exact,
        baked.stats.holes_lost,
        baked.mesh.polygon_count(),
        baked.mesh.vertices.len(),
        baked.mesh.corners.len(),
        bytes.len(),
        times[0],
        times[ROUNDS - 1],
    );
    println!("  seams: {:?}", baked.seams);
    println!("  digest {}", blake3::hash(&bytes));
    if let Some(out) = out {
        let path = out.join(format!("{name}.snav"));
        std::fs::write(&path, &*bytes).expect("it saves");
    }
}

/// What an edit to one tile costs: its bake alone, and the stitch that follows, over every tile.
fn one_tile(triangles: &[Triangle], half: f32, settings: &BakeSettings) {
    let bounds = around_origin(half, settings);
    let mut tiles = Vec::new();
    let mut worst = (0.0, [0, 0], 0);
    for z in bounds.0[1]..=bounds.1[1] {
        for x in bounds.0[0]..=bounds.1[0] {
            let reaches: Vec<Triangle> = triangles
                .iter()
                .filter(|triangle| {
                    let (low, high) = tile_range(triangle, settings);
                    low[0] <= x && x <= high[0] && low[1] <= z && z <= high[1]
                })
                .copied()
                .collect();
            let begun = Stopwatch::start();
            let tile = bake_tile(&reaches, [x, z], settings).expect("it bakes");
            let time = begun.elapsed().as_secs_f64() * 1000.0;
            if time > worst.0 {
                worst = (time, [x, z], reaches.len());
            }
            tiles.extend(tile);
        }
    }
    let begun = Stopwatch::start();
    stitch(&tiles, settings).expect("it stitches");
    println!(
        "  slowest tile {:?}: {:.1} ms over {} triangles; the stitch of {} tiles {:.2} ms",
        worst.1,
        worst.0,
        worst.2,
        tiles.len(),
        begun.elapsed().as_secs_f64() * 1000.0,
    );
}

/// The town and its streets; past this is only ground.
const TOWN_HALF: f32 = 96.0;

fn main() {
    let out = std::env::args().nth(1).map(PathBuf::from);
    let begun = Stopwatch::start();
    let arena = nav_support::arena_triangles();
    let town = nav_support::town_triangles();
    println!("geometry read in {:.0} ms", begun.elapsed().as_secs_f64() * 1000.0);

    let settings = BakeSettings::CHARACTER;
    run("arena", &arena, 64.0, &settings, out.as_ref());
    run("town", &town, TOWN_HALF, &settings, out.as_ref());
    one_tile(&town, TOWN_HALF, &settings);

    // What the choice of tile and cell costs.
    let eight = BakeSettings { tile_cells: 64, ..settings };
    run("town-8m", &town, TOWN_HALF, &eight, None);
    let twenty = BakeSettings { tile_cells: 160, ..settings };
    run("town-20m", &town, TOWN_HALF, &twenty, None);
    let fine = BakeSettings { cell_size: 0.0625, ..settings };
    run("town-fine", &town, TOWN_HALF, &fine, None);
}
