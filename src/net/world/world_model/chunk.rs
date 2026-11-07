use std::num::NonZeroU16;

use bitcode::{Decode, Encode};

use super::{
    CHUNK_AREA, ChunkData,
    encoding::{PixelRunner, RawPixel},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Encode, Decode)]
pub enum PixelFlags {
    /// Actual material isn't known yet.
    #[default]
    Unknown,
    Normal,
    Fluid,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Encode, Decode)]
pub struct Pixel {
    pub flags: PixelFlags,
    pub material: u16,
}

impl Pixel {
    /// Materials must fit in 11 bits once incremented, so ids at or above this
    /// cannot be represented in the compact form.
    pub const MAX_MATERIAL: u16 = 2046;

    pub fn to_raw(self) -> RawPixel {
        RawPixel {
            material: if self.flags != PixelFlags::Unknown {
                self.material
            } else {
                u16::MAX
            },
            flags: if self.flags == PixelFlags::Normal {
                0
            } else {
                1
            },
        }
    }
    pub fn to_compact(self) -> CompactPixel {
        let flag_bit = if self.flags == PixelFlags::Normal {
            0
        } else {
            1
        };
        // Only 11 bits are available for the material, and the encoding stores
        // `material + 1` so that raw 0 is never produced (CompactPixel is a
        // NonZeroU16). Ids at or above the limit used to wrap: 2047+Normal
        // produced raw 0 and panicked the unwrap, 2046+Fluid collided with
        // UNKNOWN_RAW, and anything >= 2048 silently aliased mod 2048. Encode
        // them as Unknown instead - wrong, but neither a crash nor a silent
        // impersonation of an unrelated material.
        let raw = if self.flags == PixelFlags::Unknown || self.material >= Self::MAX_MATERIAL {
            CompactPixel::UNKNOWN_RAW
        } else {
            ((self.material + 1) << 1) | flag_bit
        };
        CompactPixel(NonZeroU16::new(raw).unwrap())
    }
    fn from_compact(compact: CompactPixel) -> Self {
        let raw = u16::from(compact.0);
        // Must be checked before the arithmetic below: UNKNOWN_RAW is 4095, and
        // `(4095 >> 1) - 1` is fine, but raw values of 0 or 1 would underflow.
        if raw == CompactPixel::UNKNOWN_RAW || raw < 2 {
            return Pixel {
                flags: PixelFlags::Unknown,
                material: 0,
            };
        }
        let material = (raw >> 1) - 1;
        let flags = if raw & 1 == 1 {
            PixelFlags::Fluid
        } else {
            PixelFlags::Normal
        };
        Pixel { flags, material }
    }
}

/// An entire pixel packed into 12 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
#[repr(transparent)]
pub struct CompactPixel(pub NonZeroU16);

impl CompactPixel {
    const UNKNOWN_RAW: u16 = 4095;
    fn from_raw(val: u16) -> Self {
        CompactPixel(NonZeroU16::new(val).unwrap())
    }
    fn raw(self) -> u16 {
        u16::from(self.0)
    }
}

impl Default for CompactPixel {
    fn default() -> Self {
        Self(NonZeroU16::new(CompactPixel::UNKNOWN_RAW).unwrap())
    }
}

/// Chunk of pixels. Stores pixels and tracks if they were changed.
pub struct Chunk {
    pixels: [u16; CHUNK_AREA],
    // A plain bool array, deliberately: a bitset is smaller, but measures ~6x
    // slower for this per-pixel access pattern.
    changed: [bool; CHUNK_AREA],
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            pixels: [CompactPixel::UNKNOWN_RAW; CHUNK_AREA],
            changed: [false; CHUNK_AREA],
        }
    }
}

impl Chunk {
    pub fn pixel(&self, offset: usize) -> Pixel {
        Pixel::from_compact(CompactPixel::from_raw(self.pixels[offset]))
    }

    pub fn compact_pixel(&self, offset: usize) -> CompactPixel {
        CompactPixel::from_raw(self.pixels[offset])
    }

    pub fn set_pixel(&mut self, offset: usize, pixel: Pixel) {
        let px = pixel.to_compact().raw();
        if self.pixels[offset] != px {
            self.pixels[offset] = px;
            self.mark_changed(offset);
        }
    }

    pub fn set_compact_pixel(&mut self, offset: usize, pixel: CompactPixel) {
        let px = pixel.raw();
        if self.pixels[offset] != px {
            self.pixels[offset] = px;
            self.mark_changed(offset);
        }
    }
    pub fn changed(&self, offset: usize) -> bool {
        self.changed[offset]
    }

    pub fn mark_changed(&mut self, offset: usize) {
        self.changed[offset] = true;
    }

    pub fn clear_changed(&mut self) {
        self.changed = [false; CHUNK_AREA];
    }

    pub fn to_chunk_data(&self) -> ChunkData {
        let mut runner = PixelRunner::new();
        for i in 0..CHUNK_AREA {
            runner.put_pixel(self.compact_pixel(i))
        }
        let runs = runner.build();
        ChunkData { runs }
    }
}
