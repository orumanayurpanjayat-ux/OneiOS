// SPDX-License-Identifier: MIT
//! The helper's own copy of the guest's frame (B,G,R,X), and the hash of each tile: the picture the layer is to show,
//! which every post fills its buffer from. [`Snapshot::update`] reads each guest row once into `row`, hashes the
//! tiles from that stable copy, and copies a tile into `pixels` only when its hash differs. The hash therefore always
//! describes the bytes copied into the snapshot. Wide frames use up to 24 horizontal tiles (60 pixels at the S24
//! Ultra's 1440-pixel width); small frames keep the original four-tile layout. A tile's hash covers B,G,R, not X.
//! `full`: every pixel is the newest picture's (the first update or put of the whole frame makes it so).
use super::direct::Direct;
use super::protocol::FrameRef;
use super::{Rect, FORMAT_BGRX};
use crate::sys::{Errno, Result};
use xh_core::bytes::le;

pub const TILES: u32 = 4;
const WIDE_TILES: u32 = 24;
const WIDE_WIDTH: u32 = 256;

pub struct Snapshot {
    pixels: Vec<u8>,
    row: Vec<u8>,
    hash: Vec<u64>,
    pub stride: usize,
    pub width: u32,
    pub height: u32,
    pub tile: u32,
    tiles_x: u32,
    pub full: bool,
}

/// A zeroed vector of n items, or ENOMEM (the snapshot is 18 MB at 1440x3120: no memory is a case the helper handles).
fn zeroed<T: Copy + Default>(n: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Errno(libc::ENOMEM))?;
    v.resize(n, T::default());
    Ok(v)
}

impl Snapshot {
    /// ENOMEM for a size of 0 or no memory.
    pub fn new(width: u32, height: u32) -> Result<Snapshot> {
        if width == 0 || height == 0 {
            return Err(Errno(libc::ENOMEM));
        }
        let stride = width as usize * 4;
        let tile = if width >= WIDE_WIDTH { width.div_ceil(WIDE_TILES) } else { width.div_ceil(TILES) };
        let tiles_x = width.div_ceil(tile);
        Ok(Snapshot {
            pixels: zeroed(height as usize * stride)?,
            row: zeroed(stride)?,
            hash: zeroed(height as usize * tiles_x as usize)?,
            stride,
            width,
            height,
            tile,
            tiles_x,
            full: false,
        })
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Reads each complete row of the pending direct frame once, then hashes and copies its changed tiles from the
    /// row scratch. Returns the bounding rectangle of changed tiles (empty: the snapshot already holds the frame) and
    /// an error of Direct::copy_direct's kind. Rows completed before a fault are in the snapshot; the incomplete row
    /// is ignored.
    pub fn update(&mut self, d: &Direct) -> (Rect, Result<()>) {
        let mut k = match d.walk() {
            Ok(k) => k,
            Err(e) => return (Rect::EMPTY, Err(e)),
        };
        let width = k.s.width.min(self.width);
        let height = k.s.height.min(self.height);
        let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
        let mut result = Ok(());
        'rows: for row in 0..height {
            // Read the scanout segments once per row, rather than re-walking them for every tile. Do not publish a
            // partial row if a segment is invalid or the pending slot was reused mid-read.
            let read = &mut self.row[..width as usize * 4];
            if let Err(e) = k.row_into(row, 0, read) {
                result = Err(e);
                break 'rows;
            }
            for t in 0..self.tiles_x {
                let x = t * self.tile;
                let w = if x >= width { 0 } else { (width - x).min(self.tile) };
                if w == 0 {
                    continue;
                }
                let at = x as usize * 4;
                let tile_bytes = &read[at..at + w as usize * 4];
                let h = hash_tile(tile_bytes, w);
                let slot = &mut self.hash[row as usize * self.tiles_x as usize + t as usize];
                if *slot == h {
                    continue;
                }
                *slot = h;
                let to = row as usize * self.stride + at;
                self.pixels[to..to + tile_bytes.len()].copy_from_slice(tile_bytes);
                x0 = x0.min(x);
                x1 = x1.max(x + w);
                y0 = y0.min(row);
                y1 = row + 1;
            }
        }
        let changed = if y0 != u32::MAX { Rect::new(x0, y0, x1 - x0, y1 - y0) } else { Rect::EMPTY };
        // Whole only when the guest's frame covers the snapshot: a smaller frame leaves the rest unknown.
        if result.is_ok() && !self.full {
            self.full = width == self.width && height == self.height;
        }
        (changed, result)
    }

    /// xnuhost's own pixels (the memfd, B,G,R,X) over rectangle r of the snapshot; those tiles' hashes are dropped, so
    /// the next direct frame copies them again. EINVAL for another format.
    pub fn put(&mut self, f: &FrameRef<'_>, mut r: Rect) -> Result<()> {
        if f.pixels.is_empty() || f.format != FORMAT_BGRX {
            return Err(Errno(libc::EINVAL));
        }
        if !r.clip(self.width.min(f.width), self.height.min(f.height)) {
            return Ok(());
        }
        let bytes = r.w as usize * 4;
        for row in r.y..r.y + r.h {
            let to = row as usize * self.stride + r.x as usize * 4;
            let from = row as usize * f.stride as usize + r.x as usize * 4;
            self.pixels[to..to + bytes].copy_from_slice(&f.pixels[from..from + bytes]);
            let last = ((r.x + r.w - 1) / self.tile).min(self.tiles_x - 1);
            for t in r.x / self.tile..=last {
                self.hash[row as usize * self.tiles_x as usize + t as usize] = 0;
            }
        }
        if r == Rect::whole(self.width, self.height) {
            self.full = true;
        }
        Ok(())
    }
}

/// Four lanes of two pixels each (one 64-bit word, the X bytes masked off), a rotate and a multiply a step: the four
/// chains run side by side, and the rotate brings every bit of a change into the low bits the next multiply spreads
/// up (a multiply alone only carries a change upwards: two pixels' high bytes could then cancel in 16 bits).
#[inline]
fn hash_step(lane: u64, w: u64) -> u64 {
    let lane = lane.wrapping_add((w & 0x00ff_ffff_00ff_ffff).wrapping_add(1));
    lane.rotate_left(23).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// The hash of a tile of `pixels` B,G,R,X pixels at the start of p. Never 0 (0: no hash, a tile xnuhost drew or not
/// read yet).
pub fn hash_tile(p: &[u8], pixels: u32) -> u64 {
    let (mut a, mut b, mut c, mut e) = (1u64, 2u64, 3u64, 4u64);
    let pixels = pixels as usize;
    let mut i = 0;
    while i + 8 <= pixels {
        let at = i * 4;
        a = hash_step(a, le::u64(p, at));
        b = hash_step(b, le::u64(p, at + 8));
        c = hash_step(c, le::u64(p, at + 16));
        e = hash_step(e, le::u64(p, at + 24));
        i += 8;
    }
    while i + 2 <= pixels {
        a = hash_step(a, le::u64(p, i * 4));
        i += 2;
    }
    if i < pixels {
        b = hash_step(b, le::u32(p, i * 4) as u64);
    }
    let mut h = a ^ b.wrapping_mul(3) ^ c.wrapping_mul(5) ^ e.wrapping_mul(7) ^ ((pixels as u64) << 48);
    h ^= h >> 31;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 29;
    h | 1
}
