use bitcode::{Decode, Encode};

/// Position and size of a world update. w/h are actually width/height - 1.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub x: i32,
    pub y: i32,
    pub w: u8,
    pub h: u8,
}

/// Pixels in the format the mod sends and expects: a header followed by pixel runs.
#[derive(Debug)]
pub struct NoitaWorldUpdate {
    pub header: Header,
    pub runs: Vec<PixelRun<RawPixel>>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RawPixel {
    pub material: u16,
    pub flags: u8,
}

/// Stores a run of pixels.
/// Not specific to Noita side - length is an actual length
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct PixelRun<T> {
    pub length: u32,
    pub data: T,
}

/// Converts a normal sequence of pixels to a run-length-encoded one.
pub struct PixelRunner<T> {
    current_pixel: Option<T>,
    current_run_len: u32,
    runs: Vec<PixelRun<T>>,
}

impl<T: Eq + Copy> Default for PixelRunner<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Eq + Copy> PixelRunner<T> {
    pub fn new() -> Self {
        Self {
            current_pixel: None,
            current_run_len: 0,
            runs: Vec::new(),
        }
    }
    pub fn put_pixel(&mut self, pixel: T) {
        if let Some(current) = self.current_pixel {
            if pixel != current {
                self.runs.push(PixelRun {
                    length: self.current_run_len,
                    data: current,
                });
                self.current_pixel = Some(pixel);
                self.current_run_len = 1;
            } else {
                self.current_run_len += 1;
            }
        } else {
            self.current_pixel = Some(pixel);
            self.current_run_len = 1;
        }
    }
    pub fn build(mut self) -> Vec<PixelRun<T>> {
        if self.current_run_len > 0 {
            self.runs.push(PixelRun {
                length: self.current_run_len,
                data: self.current_pixel.expect("has current pixel"),
            });
        }
        self.runs
    }
}

impl PixelRunner<RawPixel> {
    /// Note: w/h are actualy width/height -1
    pub fn into_noita_update(self, x: i32, y: i32, w: u8, h: u8) -> NoitaWorldUpdate {
        NoitaWorldUpdate {
            header: Header { x, y, w, h },
            runs: self.build(),
        }
    }
}

/// Reads little-endian values off the front of a byte slice.
struct ByteParser<'a> {
    data: &'a [u8],
}

impl ByteParser<'_> {
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let (bytes, rest) = self
            .data
            .split_first_chunk()
            .expect("world update is complete");
        self.data = rest;
        *bytes
    }
    fn u8(&mut self) -> u8 {
        self.bytes::<1>()[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.bytes())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.bytes())
    }
}

impl NoitaWorldUpdate {
    pub fn load(data: &[u8]) -> Self {
        let mut parser = ByteParser { data };
        let header = Header {
            x: parser.i32(),
            y: parser.i32(),
            w: parser.u8(),
            h: parser.u8(),
        };
        let run_count = parser.u16();
        let runs = (0..run_count)
            .map(|_| PixelRun {
                length: u32::from(parser.u16()) + 1,
                data: RawPixel {
                    material: parser.u16(),
                    flags: parser.u8(),
                },
            })
            .collect();
        assert!(parser.data.is_empty());
        Self { header, runs }
    }

    pub fn save(&self) -> Vec<u8> {
        let header = self.header;
        let mut buf = Vec::with_capacity(12 + self.runs.len() * 5);
        buf.extend(header.x.to_le_bytes());
        buf.extend(header.y.to_le_bytes());
        buf.push(header.w);
        buf.push(header.h);
        buf.extend((self.runs.len() as u16).to_le_bytes());
        for run in &self.runs {
            buf.extend(u16::try_from(run.length - 1).unwrap().to_le_bytes());
            buf.extend(run.data.material.to_le_bytes());
            buf.push(run.data.flags);
        }
        buf
    }
}
