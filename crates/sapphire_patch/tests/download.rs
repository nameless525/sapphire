//! The download against a localhost host: what crosses the wire for a real patch, the releases it refuses,
//! and each way a host can misbehave.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::server::{Fault, Host, STALL};
use common::{
    APP_BUILD, EXPIRES, Expected, NOW, TRUSTED, TestDir, UNTRUSTED, asked_of, assert_staged_whole,
    first_release, is_table_request, retries, second_release, staged_hashes,
};
use sapphire_assets::manifest::ManifestError;
use sapphire_patch::{
    Download, DownloadError, FetchError, HttpTransport, MANIFEST_PATH, Outcome, Phase, PlanError, Request,
    Roots, Timeouts,
};

/// Timeouts short enough that a stall costs a test a moment, not a minute.
fn timeouts() -> Timeouts {
    let quick = Duration::from_millis(400);
    Timeouts { connect: quick, response: quick, read: quick, ..Timeouts::default() }
}

fn download(host: &Host, staging: &TestDir) -> Download {
    let request = Request {
        installed: first_release().installed(1),
        staging: staging.0.clone(),
        now: NOW,
        app_build: APP_BUILD,
        retries: retries(),
    };
    let transport = HttpTransport::new(&host.url(), Roots::Platform, timeouts()).expect("a host");
    Download::start(request, transport).expect("starts")
}

/// The host of release 2 over release 1, signed as given.
fn patch_host(version: u64, expires: u64, signer: u8) -> Host {
    let next = second_release();
    Host::start(next.host(&next.signed(version, expires, signer)))
}

fn staged(outcome: Result<Outcome, DownloadError>) -> sapphire_patch::Staged {
    match outcome {
        Ok(Outcome::Staged(staged)) => staged,
        other => panic!("expected a staged release, got {other:?}"),
    }
}

#[test]
fn a_patch_moves_only_the_manifest_the_changed_tables_and_the_new_blocks() {
    let (from, to) = (first_release(), second_release());
    let signed = to.signed(2, EXPIRES, TRUSTED);
    let host = Host::start(to.host(&signed));
    let dir = TestDir::new("exact");
    let patcher = download(&host, &dir);
    let first = staged(patcher.join());
    assert_eq!(first.manifest, to.manifest(2, EXPIRES));

    let expected = Expected::between(&from, &to);
    let log = host.log();
    // One request for the manifest, two for each changed pack's tables (its header, then its table of
    // contents), and the blocks in runs of neighbours.
    assert_eq!(log[0].asked.path, MANIFEST_PATH);
    let (_, tables) = asked_of(&to, &log);
    let mut expected_tables: Vec<_> = expected.tables.iter().flat_map(|(hash, _)| [*hash, *hash]).collect();
    expected_tables.sort_unstable();
    let mut tables = tables;
    tables.sort_unstable();
    assert_eq!(tables, expected_tables);
    // Every block asked for is a new one, or padding of at most 15 bytes between two new ones in one pack.
    let mut covered = 0;
    let mut padding = 0;
    for served in log.iter().filter(|served| served.asked.path.starts_with("packs/")) {
        let range = served.asked.range.clone().expect("a range");
        if is_table_request(&range) {
            continue;
        }
        let inside: Vec<_> = expected
            .blocks
            .iter()
            .filter(|(hash, block)| {
                served.asked.path == sapphire_patch::pack_path(hash)
                    && block.start >= range.start
                    && block.end <= range.end
            })
            .map(|(_, block)| block.clone())
            .collect();
        assert!(!inside.is_empty(), "{:?} asks for no new block", served.asked);
        assert_eq!((inside[0].start, inside[inside.len() - 1].end), (range.start, range.end));
        for pair in inside.windows(2) {
            assert!(pair[1].start - pair[0].end <= 15, "only padding lies between two blocks of one request");
            padding += pair[1].start - pair[0].end;
        }
        covered += inside.len();
    }
    assert_eq!(covered, expected.blocks.len(), "every new block, once");
    let wire =
        signed.len() as u64 + expected.table_bytes() + expected.block_bytes() + padding + expected.file_bytes;
    assert_eq!(host.sent(), wire);
    assert_eq!(first.received, wire);
    assert!(padding <= 15 * expected.blocks.len() as u64);
    // The texture's two edited blocks are new and its eight others kept, and the mesh's four are new; the
    // regenerated sound's one block is new and the other sound's two kept; the new pack holding only an
    // installed sound costs its tables alone, and the fonts' pack, the same, nothing.
    let new_blocks = |pack: &str| {
        let hash = common::header(&to.packs[pack]).hash();
        expected.blocks.iter().filter(|(of, _)| *of == hash).count()
    };
    assert_eq!(new_blocks("shared"), 2 + 4);
    assert_eq!(new_blocks("audio"), 1);
    assert_eq!(new_blocks("audio_extra"), 0);
    assert!(!expected.tables.iter().any(|(hash, _)| *hash == common::header(&to.packs["fonts"]).hash()));
    eprintln!(
        "patch: {} bytes = manifest {} + tables {} + blocks {} ({} of them) + padding {padding} + files {}",
        wire,
        signed.len(),
        expected.table_bytes(),
        expected.block_bytes(),
        expected.blocks.len(),
        expected.file_bytes
    );
    assert_staged_whole(&dir.0, &from, &to);

    // Again over the same staging: nothing but the manifest crosses.
    let before = host.sent();
    let again = staged(download(&host, &dir).join());
    assert_eq!(host.sent() - before, signed.len() as u64);
    assert_eq!(again.total, first.total);
}

#[test]
fn progress_reads_done_and_total_without_blocking() {
    let host = patch_host(2, EXPIRES, TRUSTED);
    host.set_faults(|_| Fault::Delay(10));
    let dir = TestDir::new("progress");
    let patcher = download(&host, &dir);
    let mut seen = Vec::new();
    while !patcher.is_finished() {
        let progress = patcher.progress();
        assert!(progress.done <= progress.total || progress.phase == Phase::Checking, "{progress:?}");
        seen.push(progress);
        std::thread::sleep(Duration::from_millis(3));
    }
    let last = patcher.progress();
    let staged = staged(patcher.join());
    assert_eq!((last.phase, last.done, last.total), (Phase::Stopped, staged.total, staged.total));
    assert!(seen.windows(2).all(|pair| pair[0].done <= pair[1].done), "done only grows");
    assert!(seen.iter().any(|progress| progress.phase == Phase::Fetching && progress.done < progress.total));
}

#[test]
fn an_install_that_is_the_hosts_release_fetches_only_the_manifest() {
    let from = first_release();
    let signed = from.signed(1, EXPIRES, TRUSTED);
    let host = Host::start(from.host(&signed));
    let dir = TestDir::new("current");
    assert_eq!(download(&host, &dir).join().expect("checks"), Outcome::UpToDate { version: 1 });
    assert_eq!(host.sent(), signed.len() as u64);
}

/// Refused before anything but the manifest is fetched, and nothing staged.
fn assert_refused(host: &Host, name: &str) -> DownloadError {
    let dir = TestDir::new(name);
    let error = download(host, &dir).join().expect_err("refused");
    assert_eq!(host.log().len(), 1, "only the manifest was fetched");
    assert!(!dir.join("release.smanifest").exists(), "nothing was staged");
    error
}

#[test]
fn an_older_release_is_refused() {
    let host = patch_host(0, EXPIRES, TRUSTED);
    let error = assert_refused(&host, "older");
    assert!(
        matches!(error, DownloadError::Refused(ManifestError::NotNewer { installed: 1, offered: 0 })),
        "{error}"
    );
}

#[test]
fn an_expired_release_is_refused() {
    let host = patch_host(2, NOW, TRUSTED);
    let error = assert_refused(&host, "expired");
    assert!(matches!(error, DownloadError::Refused(ManifestError::Expired { version: 2, .. })), "{error}");
}

#[test]
fn a_release_signed_by_a_key_the_install_doesnt_name_is_refused() {
    let host = patch_host(2, EXPIRES, UNTRUSTED);
    let error = assert_refused(&host, "untrusted");
    assert!(matches!(error, DownloadError::Refused(ManifestError::Untrusted)), "{error}");
}

#[test]
fn a_manifest_with_a_bad_signature_or_a_flipped_byte_is_refused() {
    let next = second_release();
    let signed = next.signed(2, EXPIRES, TRUSTED);
    let mut bad_signature = signed.clone();
    *bad_signature.last_mut().expect("a signature") ^= 1;
    let host = Host::start(next.host(&bad_signature));
    let error = assert_refused(&host, "bad-signature");
    assert!(matches!(error, DownloadError::Refused(ManifestError::BadSignature { .. })), "{error}");

    let mut flipped = signed;
    flipped[20] ^= 0x40;
    let host = Host::start(next.host(&flipped));
    let error = assert_refused(&host, "flipped");
    assert!(matches!(error, DownloadError::Refused(_)), "{error}");
}

#[test]
fn a_manifest_past_the_most_a_manifest_may_be_is_cut_off_at_that() {
    let next = second_release();
    let mut files = next.host(&[]);
    files.insert(MANIFEST_PATH.to_owned(), vec![0x5a; 3 << 20].into());
    let host = Host::start(files);
    let dir = TestDir::new("endless");
    let patcher = download(&host, &dir);
    while !patcher.is_finished() {
        std::thread::sleep(Duration::from_millis(2));
    }
    let received = patcher.progress().received;
    let error = patcher.join().expect_err("refused");
    let limit = sapphire_assets::manifest::MAX_LEN;
    assert!(
        matches!(
            &error,
            DownloadError::Host { path, last: FetchError::TooLong { limit: cut }, .. }
                if path == MANIFEST_PATH && *cut == limit
        ),
        "{error}"
    );
    // Each try kept no more than the limit.
    assert_eq!(received, u64::from(retries().attempts) * limit as u64);
}

#[test]
fn a_release_that_changes_what_the_simulation_reads_is_refused() {
    let mut next = second_release();
    next.files.insert("documents/game.flow.ron".to_owned(), (b"(start: boss)".to_vec(), false));
    let host = Host::start(next.host(&next.signed(2, EXPIRES, TRUSTED)));
    let error = assert_refused(&host, "simulation");
    assert!(matches!(error, DownloadError::NotPatchable(PlanError::NotPatchable { .. })), "{error}");
}

/// A host that `fault`s every third request of the first `times` from the second on (the manifest's
/// own fetch is served, so the fault lands on tables, files and blocks) still gives the whole release, and
/// the faults happened.
fn survives(fault: Fault, times: usize) {
    let (from, to) = (first_release(), second_release());
    let host = Host::start(to.host(&to.signed(2, EXPIRES, TRUSTED)));
    let faulted = Arc::new(AtomicUsize::new(0));
    let counter = faulted.clone();
    host.set_faults(move |asked| {
        if asked.index % 3 == 1 && counter.load(Ordering::Relaxed) < times {
            counter.fetch_add(1, Ordering::Relaxed);
            fault
        } else {
            Fault::None
        }
    });
    let dir = TestDir::new(&format!("{fault:?}"));
    let staged = staged(download(&host, &dir).join());
    assert_eq!(faulted.load(Ordering::Relaxed), times, "{fault:?} hit as many requests as asked");
    assert_eq!(staged.manifest, to.manifest(2, EXPIRES));
    assert_staged_whole(&dir.0, &from, &to);
}

#[test]
fn a_dropped_connection_is_tried_again() {
    survives(Fault::Drop, 4);
}

#[test]
fn a_truncated_body_is_tried_again() {
    survives(Fault::Truncate, 4);
}

#[test]
fn corrupt_bytes_are_never_kept() {
    survives(Fault::Corrupt, 4);
}

#[test]
fn a_host_that_ignores_a_range_now_and_then_is_tried_again() {
    survives(Fault::IgnoreRange, 4);
}

#[test]
fn a_host_that_sends_too_much_is_cut_off_and_tried_again() {
    survives(Fault::Extra, 4);
}

#[test]
fn a_stall_times_out_and_is_tried_again() {
    survives(Fault::Stall, 2);
}

#[test]
fn a_host_that_always_ignores_ranges_is_refused_and_a_good_one_resumes() {
    let (from, to) = (first_release(), second_release());
    let host = Host::start(to.host(&to.signed(2, EXPIRES, TRUSTED)));
    host.set_faults(|asked| if asked.index == 0 { Fault::None } else { Fault::IgnoreRange });
    let dir = TestDir::new("ignores-ranges");
    let patcher = download(&host, &dir);
    while !patcher.is_finished() {
        std::thread::sleep(Duration::from_millis(2));
    }
    let received = patcher.progress().received;
    let error = patcher.join().expect_err("gives up");
    match &error {
        DownloadError::Host { path, attempts, last: FetchError::RangeIgnored } => {
            assert!(path.starts_with("packs/"), "{path}");
            assert_eq!(*attempts, retries().attempts);
        }
        other => panic!("{other}"),
    }
    // Each ignored range was refused at its headers: the patcher read the manifest and nothing of the
    // whole files the host began to send.
    assert_eq!(received, host.log()[0].sent);

    host.set_faults(|_| Fault::None);
    staged(download(&host, &dir).join());
    assert_staged_whole(&dir.0, &from, &to);
}

#[test]
fn a_host_that_keeps_sending_wrong_bytes_is_refused_with_what_it_sent() {
    let host = patch_host(2, EXPIRES, TRUSTED);
    host.set_faults(|asked| if asked.path.starts_with("files/") { Fault::Corrupt } else { Fault::None });
    let dir = TestDir::new("always-corrupt");
    let error = download(&host, &dir).join().expect_err("gives up");
    assert!(
        matches!(
            &error,
            DownloadError::Host { path, last: FetchError::Mismatch, .. } if path.starts_with("files/")
        ),
        "{error}"
    );
}

#[test]
fn a_cancelled_download_stops_and_the_next_fetches_nothing_twice() {
    let (from, to) = (first_release(), second_release());
    let host = Host::start(to.host(&to.signed(2, EXPIRES, TRUSTED)));
    host.set_faults(|_| Fault::Delay(15));
    let dir = TestDir::new("cancel");
    let patcher = download(&host, &dir);
    while patcher.progress().phase != Phase::Fetching
        || patcher.progress().done * 2 < patcher.progress().total
    {
        assert!(!patcher.is_finished(), "cancelled before it finished");
        std::thread::sleep(Duration::from_millis(2));
    }
    patcher.cancel();
    assert!(matches!(patcher.join(), Err(DownloadError::Cancelled)));
    let (staged_blocks, staged_tables) = staged_hashes(&dir.0);
    assert!(!staged_blocks.is_empty() && !staged_tables.is_empty(), "the first run kept something");
    let first_run = host.log().len();

    host.set_faults(|_| Fault::None);
    staged(download(&host, &dir).join());
    assert_staged_whole(&dir.0, &from, &to);
    let (blocks, tables) = asked_of(&to, &host.log()[first_run..]);
    assert!(
        blocks.iter().all(|hash| !staged_blocks.contains(hash)),
        "a block staged before was fetched again"
    );
    assert!(
        tables.iter().all(|hash| !staged_tables.contains(hash)),
        "a table staged before was fetched again"
    );
    assert!(!blocks.is_empty(), "the second run fetched the rest");
}

#[test]
fn a_staged_block_torn_on_disk_is_fetched_again_alone() {
    let (from, to) = (first_release(), second_release());
    let host = Host::start(to.host(&to.signed(2, EXPIRES, TRUSTED)));
    let dir = TestDir::new("torn");
    staged(download(&host, &dir).join());
    let blocks = std::fs::read_dir(dir.join("blocks"))
        .expect("lists")
        .map(|entry| entry.expect("an entry").path())
        .collect::<Vec<_>>();
    let torn = &blocks[0];
    let bytes = std::fs::read(torn).expect("reads");
    std::fs::write(torn, &bytes[..bytes.len() / 2]).expect("tears it");

    let before = host.log().len();
    staged(download(&host, &dir).join());
    let after = host.log();
    assert_eq!(after.len() - before, 2, "the manifest, then the torn block alone: {:?}", &after[before..]);
    assert_eq!(after[before + 1].sent, bytes.len() as u64);
    assert_staged_whole(&dir.0, &from, &to);
}

#[test]
fn a_second_download_of_one_staging_directory_waits_its_turn() {
    let host = patch_host(2, EXPIRES, TRUSTED);
    host.set_faults(|_| Fault::Delay(20));
    let dir = TestDir::new("busy");
    let first = download(&host, &dir);
    while host.log_so_far().is_empty() {
        std::thread::sleep(Duration::from_millis(2));
    }
    let error = download(&host, &dir).join().expect_err("busy");
    assert!(matches!(error, DownloadError::Staging(sapphire_patch::StagingError::Busy { .. })), "{error}");
    staged(first.join());
}

#[test]
fn a_stall_longer_than_the_tries_stops_with_a_timeout() {
    let host = patch_host(2, EXPIRES, TRUSTED);
    host.set_faults(|asked| if asked.index == 0 { Fault::None } else { Fault::Stall });
    let dir = TestDir::new("stalled");
    let error = download(&host, &dir).join().expect_err("gives up");
    assert!(matches!(error, DownloadError::Host { last: FetchError::Timeout(_), .. }), "{error}");
    // Stalled connections close on their own.
    std::thread::sleep(STALL);
}

#[test]
fn a_host_that_dribbles_a_body_is_cut_off_and_stops_with_a_timeout() {
    // Each byte arrives inside the read timeout, so only a floor on the body's rate ends the request: without
    // one, a host (or anyone on the path) holds the boot's update, and so the boot, for as long as it likes.
    let host = patch_host(2, EXPIRES, TRUSTED);
    host.set_faults(|asked| if asked.path == MANIFEST_PATH { Fault::Dribble } else { Fault::None });
    let dir = TestDir::new("dribbled");
    let error = download(&host, &dir).join().expect_err("gives up");
    match &error {
        DownloadError::Host { path, last: FetchError::Timeout(_), .. } => assert_eq!(path, MANIFEST_PATH),
        other => panic!("{other}"),
    }
}
