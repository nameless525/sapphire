//! Live link over loopback TCP: a device half serving an asset server, a time-of-day timeline and
//! cvars, as an app does once a frame, and the desktop half's client pushing to it from another
//! thread.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_assets::{
    AssetBudgets, AssetId, AssetKind, AssetServer, AssetSlot, Handle, LoadPriority, MemoryPackSource,
};
use sapphire_core::cvar::{AnyCvar, Cvar, CvarRegistry};
use sapphire_livelink::protocol::{DeviceStatus, Preamble, ReplyBody};
use sapphire_livelink::{
    Client, LinkError, LinkMode, LiveLink, LivePacks, PROTOCOL_VERSION, Push, ReloadTracker, set_cvar,
};
use sapphire_time::Stopwatch;
use sapphire_world::tod::TodTimeline;

const TEXTURE: AssetId = AssetId::from_bytes([1; 16]);
const MATERIAL: AssetId = AssetId::from_bytes([2; 16]);
const ZONE_TEXTURE: AssetId = AssetId::from_bytes([9; 16]);

static EXPOSURE: Cvar<f32> = Cvar::with_range("test.exposure", 0.0, -4.0, 4.0, "Exposure, in stops.");

/// A cooked RGBA8 texture, `size` texels square, from `seed`: noise, so it doesn't compress.
fn texture(size: u32, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9) | 1;
    let texels: Vec<u8> = (0..size * size * 4)
        .map(|_| {
            // xorshift32.
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: size,
        height: size,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    Texture { desc, levels: vec![&texels] }.to_ktx2().unwrap()
}

/// A cooked anime character material of `base_color`, sampling [`TEXTURE`].
fn material(base_color: [f32; 4]) -> Vec<u8> {
    let document = AnimeCharacterMaterial {
        base_color,
        base_color_map: Some(TEXTURE),
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&document, &swizzles).unwrap().to_cooked().unwrap().into_vec()
}

fn packed(id: AssetId, kind: AssetKind, bytes: &[u8], dependencies: Vec<AssetId>) -> PackAsset {
    PackAsset::encode(id, kind, Codec::Lz4, bytes, dependencies, 3).unwrap()
}

/// The shared pack: the texture, and a material of `base_color` binding it.
fn shared(base_color: [f32; 4]) -> Vec<u8> {
    write_pack(vec![
        packed(TEXTURE, AssetKind::Texture, &texture(64, 1), Vec::new()),
        packed(MATERIAL, AssetKind::Material, &material(base_color), vec![TEXTURE]),
    ])
    .unwrap()
}

/// What a sandbox-like app holds: an asset server reading through live link's packs, what its
/// uploader received, a timeline and cvars.
struct App {
    server: AssetServer,
    packs: Arc<LivePacks>,
    link: LiveLink,
    tracker: ReloadTracker,
    /// Every upload: (asset, slot, generation) → its bytes.
    uploads: BTreeMap<(AssetId, u32), (AssetSlot, Vec<u8>)>,
    timeline: TodTimeline,
    cvars: CvarRegistry,
    frame: u64,
}

impl App {
    fn new(base: MemoryPackSource) -> Self {
        let packs = Arc::new(LivePacks::new(base));
        let mut server = AssetServer::new(packs.clone(), AssetBudgets::DESKTOP);
        server.mount("shared").unwrap();
        let link =
            LiveLink::start(LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0))), packs.clone()).unwrap();
        let mut cvars = CvarRegistry::new();
        cvars.register(&EXPOSURE).unwrap();
        Self {
            server,
            packs,
            link,
            tracker: ReloadTracker::default(),
            uploads: BTreeMap::new(),
            timeline: TodTimeline::default_day(),
            cvars,
            frame: 0,
        }
    }

    fn address(&self) -> SocketAddr {
        self.link.address()
    }

    /// One frame: loading and uploads, then what the link pushed, applied as the sandbox does.
    fn frame(&mut self) {
        self.frame += 1;
        self.server.update();
        let uploads = &mut self.uploads;
        self.server.upload(|upload| {
            uploads.insert((upload.id(), upload.generation()), (upload.slot(), upload.bytes().to_vec()));
            Ok(())
        });
        let changes = self.server.take_changes();
        self.tracker.record(&changes);
        self.tracker.settle(&self.server.stats());
        while let Some(push) = self.link.poll() {
            match push {
                Push::Status { reply } => reply.reply(ReplyBody::Status(DeviceStatus {
                    app: "loopback".into(),
                    frame: self.frame,
                    assets: self.server.stats().into(),
                    generations: self.tracker.generations().to_vec(),
                    ..DeviceStatus::default()
                })),
                Push::Cvar { name, value, reply } => reply.reply(set_cvar(&self.cvars, &name, &value)),
                Push::Document { origin, type_name, text, reply } => {
                    if type_name != "sapphire::world::TodTimeline" {
                        reply.refuse(format!("{origin}: no owner for `{type_name}`"));
                        continue;
                    }
                    match TodTimeline::from_ron(&text) {
                        Ok(timeline) => {
                            self.timeline = timeline;
                            reply.reply(ReplyBody::DocumentApplied { owner: "the default day".into() });
                        }
                        Err(error) => reply.refuse(format!("{origin}: {error}")),
                    }
                }
                Push::Shaders { reply, .. } => reply.refuse("no renderer here"),
                Push::Pack { pack, reply } => match pack.apply(&self.packs, &mut self.server) {
                    Ok(outcome) => self.tracker.track(reply, outcome),
                    Err(error) => reply.refuse(error.to_string()),
                },
            }
        }
    }

    /// Runs frames until `client` (on another thread) is done, and returns its result.
    fn serve<T>(&mut self, client: JoinHandle<T>) -> T {
        for _ in 0..20_000 {
            if client.is_finished() {
                return client.join().unwrap();
            }
            self.frame();
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the client didn't finish");
    }

    /// Loads `id` until it is uploaded.
    fn load(&mut self, id: AssetId) -> Handle<Material> {
        let handle = self.server.load_with(id, LoadPriority::Critical).unwrap();
        let uploads = &mut self.uploads;
        self.server
            .finish(&handle, |upload| {
                uploads.insert((upload.id(), upload.generation()), (upload.slot(), upload.bytes().to_vec()));
                Ok(())
            })
            .unwrap();
        handle
    }
}

fn on_client<T: Send + 'static>(
    address: SocketAddr,
    work: impl FnOnce(&mut Client) -> T + Send + 'static,
) -> JoinHandle<T> {
    std::thread::spawn(move || work(&mut Client::connect(address).unwrap()))
}

#[test]
fn a_pushed_material_change_reloads_behind_the_same_handle() {
    let base = MemoryPackSource::new();
    base.insert("shared", shared([1.0, 0.5, 0.5, 1.0]));
    let mut app = App::new(base);
    let handle = app.load(MATERIAL);
    assert_eq!(app.server.generation(&handle), 0);
    let (slot, _) = app.uploads[&(MATERIAL, 0)].clone();

    let edited = shared([0.2, 0.4, 1.0, 1.0]);
    let len = edited.len() as u64;
    let outcome =
        app.serve(on_client(app.address(), move |client| client.push_pack("shared", &edited).unwrap()));
    assert!(outcome.mounted);
    assert_eq!((outcome.changed, outcome.removed, outcome.reloading, outcome.len), (1, 0, 1, len));
    assert!(outcome.received < 1024, "the 16 KB texture stays on the device: sent {}", outcome.received);
    assert_eq!(outcome.reloaded.len(), 1, "{outcome:?}");
    let reloaded = outcome.reloaded[0];
    assert_eq!((reloaded.id, reloaded.kind, reloaded.generation), (MATERIAL, AssetKind::Material, 1));
    assert!(outcome.failed.is_empty());

    // The same handle, the same slot, the next generation, the new bytes.
    assert_eq!(app.server.generation(&handle), 1);
    let (new_slot, bytes) = &app.uploads[&(MATERIAL, 1)];
    assert_eq!(*new_slot, slot);
    assert_eq!(*bytes, material([0.2, 0.4, 1.0, 1.0]));
    assert!(!app.uploads.contains_key(&(TEXTURE, 1)), "the unchanged texture isn't sent again");

    // The status lists it; pushing the same pack again changes nothing and sends nothing.
    let status = app.serve(on_client(app.address(), |client| client.status().unwrap()));
    assert_eq!(status.generations, outcome.reloaded);
    let again = shared([0.2, 0.4, 1.0, 1.0]);
    let outcome =
        app.serve(on_client(app.address(), move |client| client.push_pack("shared", &again).unwrap()));
    assert_eq!((outcome.changed, outcome.reloading, outcome.received), (0, 0, 0));
    assert_eq!(app.server.generation(&handle), 1);
}

#[test]
fn a_pack_streams_in_chunks_and_serves_later_mounts() {
    let base = MemoryPackSource::new();
    base.insert("shared", shared([1.0; 4]));
    let mut app = App::new(base);
    // 1 MiB of noise: four chunks and more.
    let zone =
        write_pack(vec![packed(ZONE_TEXTURE, AssetKind::Texture, &texture(512, 7), Vec::new())]).unwrap();
    let len = zone.len() as u64;
    let outcome = app.serve(on_client(app.address(), move |client| client.push_pack("zone", &zone).unwrap()));
    assert!(!outcome.mounted, "the app hasn't mounted it");
    assert!(outcome.received > 4 * sapphire_livelink::protocol::CHUNK_LEN as u64 && outcome.received < len);
    assert_eq!(app.link.counters().received, outcome.received);

    // Mounting it later reads the pushed version.
    app.server.mount("zone").unwrap();
    let handle =
        app.server.load_with::<sapphire_assets::TextureAsset>(ZONE_TEXTURE, LoadPriority::Critical).unwrap();
    app.server.finish(&handle, |_| Ok(())).unwrap();
}

#[test]
fn a_refused_pack_keeps_the_version_the_device_had() {
    let base = MemoryPackSource::new();
    base.insert("shared", shared([1.0; 4]));
    base.insert(
        "other",
        write_pack(vec![packed(ZONE_TEXTURE, AssetKind::Texture, &texture(8, 3), Vec::new())]).unwrap(),
    );
    let mut app = App::new(base);
    app.server.mount("other").unwrap();
    // `other` taking the shared pack's texture is refused by the server: the device goes on serving
    // the `other` it had.
    let clash = write_pack(vec![packed(TEXTURE, AssetKind::Texture, &texture(8, 3), Vec::new())]).unwrap();
    let error =
        app.serve(on_client(app.address(), move |client| client.push_pack("other", &clash).unwrap_err()));
    assert!(matches!(&error, LinkError::Refused(message) if message.contains("is in both pack")), "{error}");
    assert!(app.packs.pushed().is_empty(), "the refused version is served no more");
    assert!(app.server.contains(ZONE_TEXTURE));

    // A corrupt head is refused before anything is sent.
    let mut corrupt = shared([0.0; 4]);
    corrupt[300] ^= 0xff;
    let error =
        app.serve(on_client(app.address(), move |client| client.push_pack("shared", &corrupt).unwrap_err()));
    assert!(matches!(&error, LinkError::Refused(message) if message.contains("invalid")), "{error}");
}

#[test]
fn a_broken_document_is_refused_and_the_old_one_kept() {
    let mut app = App::new({
        let base = MemoryPackSource::new();
        base.insert("shared", shared([1.0; 4]));
        base
    });
    let day = include_str!("../../sapphire_world/default_day.ron");
    assert_eq!(app.timeline, TodTimeline::default_day());

    // Data that doesn't fit the type: the envelope reads, the timeline doesn't.
    let wrong = day.replacen("time: 0.0", "time: \"midnight\"", 1);
    assert_ne!(wrong, day, "the default day starts at midnight");
    let error = app.serve(on_client(app.address(), move |client| {
        client.push_document("default_day.ron", wrong).unwrap_err()
    }));
    assert!(
        matches!(&error, LinkError::Refused(message) if message.starts_with("default_day.ron:")),
        "{error}"
    );
    assert_eq!(app.timeline, TodTimeline::default_day(), "the old timeline is kept");

    // RON that doesn't parse at all is refused by the link itself.
    let unbalanced = day[..day.len() / 2].to_owned();
    let error = app.serve(on_client(app.address(), move |client| {
        client.push_document("default_day.ron", unbalanced).unwrap_err()
    }));
    assert!(matches!(error, LinkError::Refused(_)), "{error}");
    assert_eq!(app.timeline, TodTimeline::default_day());

    // A valid edit replaces it.
    let edited = day.replacen("exposure: 0.7", "exposure: 0.9", 1);
    let expected = TodTimeline::from_ron(&edited).unwrap();
    assert_ne!(expected, TodTimeline::default_day());
    let owner = app.serve(on_client(app.address(), move |client| {
        client.push_document("default_day.ron", edited).unwrap()
    }));
    assert_eq!(owner, "the default day");
    assert_eq!(app.timeline, expected);
}

#[test]
fn cvars_are_set_by_name() {
    let mut app = App::new({
        let base = MemoryPackSource::new();
        base.insert("shared", shared([1.0; 4]));
        base
    });
    let (name, value) =
        app.serve(on_client(app.address(), |client| client.set_cvar("test.exposure", "1.5").unwrap()));
    assert_eq!((name.as_str(), value.as_str()), ("test.exposure", "1.5"));
    assert_eq!(EXPOSURE.get(), 1.5);

    let errors = app.serve(on_client(app.address(), |client| {
        [
            client.set_cvar("test.exposur", "1").unwrap_err(),
            client.set_cvar("test.exposure", "9").unwrap_err(),
        ]
    }));
    assert!(errors[0].to_string().contains("did you mean `test.exposure`"), "{}", errors[0]);
    assert!(errors[1].to_string().contains("within"), "{}", errors[1]);
    assert_eq!(EXPOSURE.get(), 1.5, "a refused value leaves the cvar as it was");
    EXPOSURE.reset();
    assert!(EXPOSURE.is_default());
}

#[test]
fn other_protocol_versions_are_refused_on_both_sides() {
    let app = App::new({
        let base = MemoryPackSource::new();
        base.insert("shared", shared([1.0; 4]));
        base
    });
    // A tool of the next version: the device answers with its own preamble, then closes.
    let mut stream = TcpStream::connect(app.address()).unwrap();
    // Before the device closes it: macOS refuses socket options on a closed connection.
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(&Preamble { version: PROTOCOL_VERSION + 1 }.to_bytes()).unwrap();
    let device = Preamble::read(&mut stream).unwrap();
    assert_eq!(device.version, PROTOCOL_VERSION);
    stream.write_all(&[0; 16]).ok();
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest);
    assert!(rest.is_empty(), "nothing after the preamble");

    // A device of the next version: the tool says which versions met.
    let device = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = device.local_addr().unwrap();
    let fake = std::thread::spawn(move || {
        let (mut stream, _) = device.accept().unwrap();
        stream.write_all(&Preamble { version: PROTOCOL_VERSION + 1 }.to_bytes()).unwrap();
        let _ = Preamble::read(&mut stream);
    });
    let error = Client::connect(address).unwrap_err();
    assert!(
        matches!(error, LinkError::Version { peer, ours } if peer == PROTOCOL_VERSION + 1 && ours == PROTOCOL_VERSION),
        "{error}"
    );
    fake.join().unwrap();
}

#[test]
fn a_device_can_dial_the_tool() {
    // `adb reverse`: the tool listens, the device dials it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base = MemoryPackSource::new();
    base.insert("shared", shared([1.0; 4]));
    let packs = Arc::new(LivePacks::new(base));
    let link = LiveLink::start(LinkMode::Connect(address), packs).unwrap();
    let tool = std::thread::spawn(move || Client::accept(&listener).unwrap().status().unwrap());
    let status = loop {
        if tool.is_finished() {
            break tool.join().unwrap();
        }
        if let Some(Push::Status { reply }) = link.poll() {
            reply.reply(ReplyBody::Status(DeviceStatus { app: "dialed".into(), ..DeviceStatus::default() }));
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(status.app, "dialed");
    drop(link);
}

#[test]
fn a_dialing_device_waits_between_dials() {
    // Behind `adb reverse` with no tool on the desktop, adbd takes every dial and closes it at
    // once: the device must not spin, and must keep dialing.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
    let link = LiveLink::start(LinkMode::Connect(address), packs).unwrap();
    // The time from closing each dial to the next. The device waits once it sees the close, so
    // however late this thread takes a dial, the next can't come sooner than the wait.
    let (mut since_close, mut gaps) = (None::<Stopwatch>, Vec::new());
    let deadline = Stopwatch::start();
    while gaps.len() < 3 && deadline.elapsed() < Duration::from_secs(60) {
        match listener.accept() {
            Ok((stream, _)) => {
                gaps.extend(since_close.as_ref().map(Stopwatch::elapsed));
                drop(stream);
                since_close = Some(Stopwatch::start());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    }
    drop(link);
    assert_eq!(gaps.len(), 3, "the device dials again after each close, within a minute: {gaps:?}");
    assert!(
        gaps.iter().all(|gap| *gap >= Duration::from_millis(950)),
        "the device waits a second between dials: {gaps:?}"
    );
}
