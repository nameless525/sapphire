# The app icon

`icon.png` is Sapphire's icon: the maintainer's own artwork, given on 2026-11-13. It's the master
every platform's icon set is made from. Replace it byte for byte as given; nothing redraws it.

A master must be:

- square, at least 1024 px a side;
- opaque, with no transparent pixel (App Store icons can't have any);
- drawn inside the middle circle, of a diameter 72/108 of the side, on a flat ground that fills the
  rest, so Android's smallest adaptive mask and iOS's rounded corners cut none of the art.

`sapphire_pipeline`'s tests hold the committed master to these rules. To make the sets:

    cargo run -p sapphire_pipeline -- icons --source platform/icon/icon.png --out <dir>

`cargo xtask package` does the same for each store build.
