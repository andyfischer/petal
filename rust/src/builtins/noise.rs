//! Perlin noise builtins (1D/2D/3D) and noise_seed.

use crate::native_fn::PetalCxt;

use super::require_args;

/// Global noise seed, set via noise_seed().
static NOISE_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Permutation table for noise, derived from seed.
fn noise_perm(seed: u64) -> [u8; 512] {
    let mut perm = [0u8; 512];
    let mut p = [0u8; 256];
    for i in 0..256 {
        p[i] = i as u8;
    }
    // Fisher-Yates shuffle with seed
    let mut rng = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..256).rev() {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = (rng >> 33) as usize % (i + 1);
        p.swap(i, j);
    }
    for i in 0..512 {
        perm[i] = p[i & 255];
    }
    perm
}

fn grad1(hash: u8, x: f64) -> f64 {
    if hash & 1 == 0 { x } else { -x }
}

fn grad2(hash: u8, x: f64, y: f64) -> f64 {
    let h = hash & 3;
    match h {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

fn grad3(hash: u8, x: f64, y: f64, z: f64) -> f64 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 { y } else if h == 12 || h == 14 { x } else { z };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn perlin_1d(x: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let xf = x - x.floor();
    let u = fade(xf);
    let a = perm[xi as usize];
    let b = perm[(xi + 1) as usize & 255];
    let g0 = grad1(a, xf);
    let g1 = grad1(b, xf - 1.0);
    g0 + u * (g1 - g0)
}

fn perlin_2d(x: f64, y: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let yi = y.floor() as i32 & 255;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let u = fade(xf);
    let v = fade(yf);

    let aa = perm[perm[xi as usize] as usize + yi as usize] as usize;
    let ab = perm[perm[xi as usize] as usize + (yi + 1) as usize & 255] as usize;
    let ba = perm[perm[(xi + 1) as usize & 255] as usize + yi as usize] as usize;
    let bb = perm[perm[(xi + 1) as usize & 255] as usize + (yi + 1) as usize & 255] as usize;

    let x1 = grad2(perm[aa], xf, yf);
    let x2 = grad2(perm[ba], xf - 1.0, yf);
    let y1 = grad2(perm[ab], xf, yf - 1.0);
    let y2 = grad2(perm[bb], xf - 1.0, yf - 1.0);

    let lerp_x1 = x1 + u * (x2 - x1);
    let lerp_x2 = y1 + u * (y2 - y1);
    lerp_x1 + v * (lerp_x2 - lerp_x1)
}

fn perlin_3d(x: f64, y: f64, z: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let yi = y.floor() as i32 & 255;
    let zi = z.floor() as i32 & 255;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let zf = z - z.floor();
    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    let a  = perm[xi as usize] as usize + yi as usize;
    let aa = perm[a & 255] as usize + zi as usize;
    let ab = perm[(a + 1) & 255] as usize + zi as usize;
    let b  = perm[((xi + 1) & 255) as usize] as usize + yi as usize;
    let ba = perm[b & 255] as usize + zi as usize;
    let bb = perm[(b + 1) & 255] as usize + zi as usize;

    let l1 = grad3(perm[aa & 511], xf, yf, zf);
    let l2 = grad3(perm[(ba) & 511], xf - 1.0, yf, zf);
    let l3 = grad3(perm[(ab) & 511], xf, yf - 1.0, zf);
    let l4 = grad3(perm[(bb) & 511], xf - 1.0, yf - 1.0, zf);
    let l5 = grad3(perm[(aa + 1) & 511], xf, yf, zf - 1.0);
    let l6 = grad3(perm[(ba + 1) & 511], xf - 1.0, yf, zf - 1.0);
    let l7 = grad3(perm[(ab + 1) & 511], xf, yf - 1.0, zf - 1.0);
    let l8 = grad3(perm[(bb + 1) & 511], xf - 1.0, yf - 1.0, zf - 1.0);

    let x1 = l1 + u * (l2 - l1);
    let x2 = l3 + u * (l4 - l3);
    let x3 = l5 + u * (l6 - l5);
    let x4 = l7 + u * (l8 - l7);
    let y1 = x1 + v * (x2 - x1);
    let y2 = x3 + v * (x4 - x3);
    y1 + w * (y2 - y1)
}

pub(super) fn native_noise(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    let seed = NOISE_SEED.load(std::sync::atomic::Ordering::Relaxed);
    let perm = noise_perm(seed);
    match argc {
        1 => {
            let x = state.get_float(1)?;
            state.push_float(perlin_1d(x, &perm));
            Ok(1)
        }
        2 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            state.push_float(perlin_2d(x, y, &perm));
            Ok(1)
        }
        3 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            let z = state.get_float(3)?;
            state.push_float(perlin_3d(x, y, z, &perm));
            Ok(1)
        }
        _ => Err("noise() expects 1-3 arguments".into()),
    }
}

pub(super) fn native_noise_seed(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "noise_seed")?;
    let seed = state.get_int(1)? as u64;
    NOISE_SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
    state.push_nil();
    Ok(1)
}
