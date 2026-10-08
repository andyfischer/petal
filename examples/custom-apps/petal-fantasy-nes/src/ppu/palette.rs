//! The fixed 64-entry master palette.
//!
//! Every color a cart can name is an index into this table — carts never pick
//! RGB. That is the single biggest reason NES-era art looks the way it does,
//! so it is a hard constraint here rather than a suggestion.
//!
//! The values are the usual RGB approximation of the 2C02's composite output
//! (the "Nestopia/FCEUX" family of tables). Entries 0x0D, 0x0E, 0x0F, 0x1E,
//! 0x1F, 0x2E, 0x2F, 0x3E, 0x3F are the hardware's blacks/unused slots and are
//! kept black so an off-by-one in a cart's color math degrades quietly.

/// Number of entries. Indices are masked into this range, never rejected.
pub const MASTER_COUNT: usize = 64;

#[rustfmt::skip]
pub const MASTER_PALETTE: [[u8; 3]; MASTER_COUNT] = [
    [ 84,  84,  84], [  0,  30, 116], [  8,  16, 144], [ 48,   0, 136],
    [ 68,   0, 100], [ 92,   0,  48], [ 84,   4,   0], [ 60,  24,   0],
    [ 32,  42,   0], [  8,  58,   0], [  0,  64,   0], [  0,  60,   0],
    [  0,  50,  60], [  0,   0,   0], [  0,   0,   0], [  0,   0,   0],

    [152, 150, 152], [  8,  76, 196], [ 48,  50, 236], [ 92,  30, 228],
    [136,  20, 176], [160,  20, 100], [152,  34,  32], [120,  60,   0],
    [ 84,  90,   0], [ 40, 114,   0], [  8, 124,   0], [  0, 118,  40],
    [  0, 102, 120], [  0,   0,   0], [  0,   0,   0], [  0,   0,   0],

    [236, 238, 236], [ 76, 154, 236], [120, 124, 236], [176,  98, 236],
    [228,  84, 236], [236,  88, 180], [236, 106, 100], [212, 136,  32],
    [160, 170,   0], [116, 196,   0], [ 76, 208,  32], [ 56, 204, 108],
    [ 56, 180, 204], [ 60,  60,  60], [  0,   0,   0], [  0,   0,   0],

    [236, 238, 236], [168, 204, 236], [188, 188, 236], [212, 178, 236],
    [236, 174, 236], [236, 174, 212], [236, 180, 176], [228, 196, 144],
    [204, 210, 120], [180, 222, 120], [168, 226, 144], [152, 226, 180],
    [160, 214, 228], [160, 162, 160], [  0,   0,   0], [  0,   0,   0],
];

/// RGB triple for a master-palette index. Out-of-range indices wrap rather
/// than panic: a cart is untrusted input, and a wrong color beats a crash.
#[inline]
pub fn rgb(index: u8) -> [u8; 3] {
    MASTER_PALETTE[(index as usize) % MASTER_COUNT]
}

/// The same color packed as `0xRRGGBB`, for the `master_rgb` native (tooling
/// and palette-preview carts want a single int, not three).
#[inline]
pub fn packed(index: u8) -> i64 {
    let c = rgb(index);
    ((c[0] as i64) << 16) | ((c[1] as i64) << 8) | (c[2] as i64)
}
