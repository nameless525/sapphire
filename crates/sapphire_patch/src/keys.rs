//! The test key (M4 P4, m4-plan §3 "Keys"): what development builds trust, so a patch can be made and served
//! on this machine with no key anyone keeps.
//!
//! Its seed is fixed and public: 32 bytes of `0x50`, which tests and `cargo xtask patch` sign with. So anyone can
//! sign with it, and a build that ships must never trust it: a shipping package refuses one whose binary or
//! release manifest holds [`TEST_KEY`] (`xtask package`). A store build trusts the maintainer's keys, compiled
//! in from `platform/patch.toml`.

use sapphire_assets::manifest::PublicKey;

/// The test key's public half: ed25519 from the seed `[0x50; 32]`.
pub const TEST_KEY: PublicKey = [
    0x3e, 0xf0, 0x89, 0x59, 0x03, 0x30, 0x28, 0xc8, 0xfc, 0x6f, 0xc6, 0xd4, 0x91, 0xa1, 0x25, 0x48, 0xe7,
    0x8d, 0xf6, 0xae, 0xfb, 0x2d, 0x7c, 0x2c, 0x19, 0x0c, 0x3f, 0x78, 0xce, 0xf3, 0xf5, 0x0b,
];

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;

    /// The seed every test and `cargo xtask patch` signs with: `SAPPHIRE_RELEASE_KEY` = 64 `5`s and `0`s.
    const TEST_SEED: [u8; 32] = [0x50; 32];

    #[test]
    fn the_test_key_is_its_seeds() {
        assert_eq!(SigningKey::from_bytes(&TEST_SEED).verifying_key().to_bytes(), TEST_KEY);
    }
}
