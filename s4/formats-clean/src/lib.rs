//! Clean-room implementations of the byte formats described in `SPEC.md`.

pub mod cipher {
    /// The key used for the game's own data.
    pub const DEFAULT_KEY: &[u8] = b"01234567890123456789";

    /// A key-stream generator; see SPEC.md section 1.
    pub struct KeystreamCipher {
        a: u32,
        b: u32,
        c: u32,
    }

    impl KeystreamCipher {
        pub fn new(key: &[u8]) -> KeystreamCipher {
            let seed: &[u8] = if key.is_empty() { b"Default Seed" } else { key };
            let mut padded = [0u8; 12];
            for (i, slot) in padded.iter_mut().enumerate() {
                *slot = seed[i % seed.len()];
            }
            fn register(bytes: &[u8]) -> u32 {
                bytes
                    .iter()
                    .fold(0u32, |r, &b| (r << 8) | (b as i8 as i32 as u32))
            }
            let mut a = register(&padded[0..4]);
            let mut b = register(&padded[4..8]);
            let mut c = register(&padded[8..12]);
            if a == 0 {
                a = 0x1357_9bdf;
            }
            if b == 0 {
                b = 0x2468_ace0;
            }
            if c == 0 {
                c = 0xfdb9_7531;
            }
            KeystreamCipher { a, b, c }
        }

        /// XORs `data` with the next bytes of the key stream.
        pub fn apply(&mut self, data: &mut [u8]) {
            for byte in data {
                let mut ob = self.b & 1;
                let mut oc = self.c & 1;
                let mut stream = 0u8;
                for _ in 0..8 {
                    if self.a & 1 == 0 {
                        self.a >>= 1;
                        if self.c & 1 == 0 {
                            self.c = (self.c >> 1) & 0x0fff_ffff;
                            oc = 0;
                        } else {
                            self.c = ((self.c ^ 0x1000_0002) >> 1) | 0xf000_0000;
                            oc = 1;
                        }
                    } else {
                        self.a = ((self.a ^ 0x8000_0062) >> 1) | 0x8000_0000;
                        if self.b & 1 == 0 {
                            self.b = (self.b >> 1) & 0x3fff_ffff;
                            ob = 0;
                        } else {
                            self.b = ((self.b ^ 0x4000_0020) >> 1) | 0xc000_0000;
                            ob = 1;
                        }
                    }
                    stream = (stream << 1) | (ob ^ oc) as u8;
                }
                *byte ^= stream;
            }
        }
    }

    /// Applies the default key stream.
    pub fn apply_default(data: &mut [u8]) {
        KeystreamCipher::new(DEFAULT_KEY).apply(data);
    }
}

pub mod compress {
    #[derive(Debug, PartialEq, Eq)]
    pub enum DecompressError {
        Truncated,
        Corrupt,
    }

    struct Bits<'a> {
        src: &'a [u8],
        pos: usize,
    }
    impl Bits<'_> {
        fn read(&mut self, n: usize) -> Option<u32> {
            if n > 32 || self.pos.checked_add(n)? > self.src.len().checked_mul(8)? {
                return None;
            }
            let mut value = 0u32;
            for _ in 0..n {
                let byte = self.src[self.pos / 8];
                value = (value << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
                self.pos += 1;
            }
            Some(value)
        }
        fn unary(&mut self) -> u32 {
            let mut n = 0;
            while let Some(bit) = self.read(1) {
                if bit == 1 {
                    break;
                }
                n += 1;
            }
            n
        }
    }

    const INITIAL_CLASSES: [(usize, u32); 16] = [
        (2, 0),
        (3, 4),
        (3, 12),
        (4, 20),
        (4, 36),
        (4, 52),
        (4, 68),
        (4, 84),
        (4, 100),
        (4, 116),
        (4, 132),
        (4, 148),
        (4, 164),
        (5, 180),
        (5, 212),
        (5, 244),
    ];

    /// Decompresses `src` into exactly `expected_len` bytes.
    pub fn decompress(src: &[u8], expected_len: usize) -> Result<Vec<u8>, DecompressError> {
        let mut bits = Bits { src, pos: 0 };
        let mut classes = INITIAL_CLASSES;
        let mut symbols: Vec<usize> = (0x100..=0x10f).collect();
        symbols.extend([0, 32, 48, 255]);
        symbols.extend((1..=254).filter(|&v| v != 32 && v != 48));
        symbols.extend([0x110, 0x111]);
        let mut freq = [0i16; 274];
        let mut window = vec![0u8; 65536];
        let mut out = Vec::with_capacity(expected_len);
        let mut w = 0usize;

        loop {
            let class = bits.read(4).ok_or(DecompressError::Truncated)? as usize;
            let (extra, first) = classes[class];
            let raw = bits.read(extra).ok_or(DecompressError::Truncated)?;
            let index = first.wrapping_add(raw);
            if index >= 274 {
                return Err(DecompressError::Corrupt);
            }
            let s = symbols[index as usize];
            freq[s] = freq[s].wrapping_add(1);
            match s {
                0..=255 => {
                    if out.len() >= expected_len {
                        return Err(DecompressError::Corrupt);
                    }
                    out.push(s as u8);
                    window[w & 65535] = s as u8;
                    w += 1;
                }
                0x100..=0x10f => {
                    let code = s - 0x100;
                    let length = if code < 8 {
                        4 + code
                    } else {
                        let (n, base) = [
                            (1, 8),
                            (2, 10),
                            (3, 14),
                            (4, 22),
                            (5, 38),
                            (6, 70),
                            (7, 134),
                            (8, 262),
                        ][code - 8];
                        4 + base + bits.read(n).ok_or(DecompressError::Truncated)? as usize
                    };
                    let k = bits.read(3).ok_or(DecompressError::Truncated)? as usize;
                    let (n, base) = [
                        (0, 0),
                        (0, 1),
                        (1, 2),
                        (2, 4),
                        (3, 8),
                        (4, 16),
                        (5, 32),
                        (6, 64),
                    ][k];
                    let high = bits.read(8).ok_or(DecompressError::Truncated)? as usize;
                    let low = bits.read(n + 1).ok_or(DecompressError::Truncated)? as usize;
                    let distance = ((high << (n + 1)) | low) + base * 512;
                    if out
                        .len()
                        .checked_add(length)
                        .filter(|&v| v <= expected_len)
                        .is_none()
                    {
                        return Err(DecompressError::Corrupt);
                    }
                    for _ in 0..length {
                        let byte = window[w.wrapping_sub(distance) & 65535];
                        out.push(byte);
                        window[w & 65535] = byte;
                        w += 1;
                    }
                }
                0x110 => {
                    let mut order: Vec<(i16, usize)> = freq
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(t, f)| (f, t))
                        .collect();
                    order.sort_unstable_by(|a, b| b.cmp(a));
                    symbols = order.into_iter().map(|(_, t)| t).collect();
                    for f in &mut freq {
                        *f >>= 1;
                    }
                    let (mut width, mut first) = (0u32, 0u32);
                    for item in &mut classes {
                        width = width.wrapping_add(bits.unary());
                        *item = (width as usize, first);
                        first = first.wrapping_add(if width < 32 { 1u32 << width } else { 0 });
                    }
                }
                0x111 => {
                    if out.len() == expected_len {
                        return Ok(out);
                    }
                    if (bits.pos + 7) / 8 >= src.len() {
                        return Err(DecompressError::Corrupt);
                    }
                    bits.pos = (bits.pos + 7) & !7;
                }
                _ => return Err(DecompressError::Corrupt),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Batch 2 (SPEC.md sections 3 to 5).

/// The error of the parsers in `format`, `texts` and `archive`.
#[derive(Debug, PartialEq, Eq)]
pub enum FormatError {
    /// A magic number or version that is not accepted (the offending value).
    BadMagic(u32),
    /// The data ends too early; `offset` is the position given in SPEC.md for each case.
    Truncated { offset: usize },
    /// An offset that does not fit the data (see SPEC.md, sound banks).
    BadOffset { index: usize, offset: u32 },
}

pub mod format {
    use super::FormatError;

    /// Size of the file header in bytes.
    pub const HEADER_SIZE: usize = 20;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FileHeader {
        pub magic: u32,
        pub version: u32,
    }

    /// SPEC.md section 3.1.
    pub fn parse_header(data: &[u8]) -> Result<FileHeader, FormatError> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Truncated { offset: 0 });
        }
        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if magic & 0xffff_fff0 != 0x8000 {
            return Err(FormatError::BadMagic(magic));
        }
        let version = u32::from_le_bytes(data[16..20].try_into().unwrap());
        Ok(FileHeader { magic, version })
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Index {
        pub header: FileHeader,
        pub offsets: Vec<u32>,
    }

    /// SPEC.md section 3.2.
    pub fn parse_index(data: &[u8]) -> Result<Index, FormatError> {
        let header = parse_header(data)?;
        let offsets = data[HEADER_SIZE..]
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        Ok(Index { header, offsets })
    }

    /// SPEC.md section 3.3.
    pub fn entry_number(offset: u32) -> Option<usize> {
        if offset < HEADER_SIZE as u32 {
            None
        } else {
            Some(((offset - HEADER_SIZE as u32) / 4) as usize)
        }
    }
}

pub mod texts {
    use super::FormatError;

    pub const VERSION: u32 = 0x15;
    pub const COUNT: usize = 0xefc;

    /// SPEC.md section 4.1.
    pub fn parse(data: &[u8]) -> Result<Vec<Vec<u8>>, FormatError> {
        if data.len() < 4 {
            return Err(FormatError::Truncated { offset: 0 });
        }
        let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if version != VERSION {
            return Err(FormatError::BadMagic(version));
        }
        let mut strings = Vec::new();
        let mut p = 4usize;
        while data.len() - p >= 4 {
            let signed = i32::from_le_bytes(data[p..p + 4].try_into().unwrap());
            p += 4;
            let n = if signed < 0 { 0 } else { signed as usize };
            if n > data.len() - p {
                return Err(FormatError::Truncated { offset: p });
            }
            strings.push(data[p..p + n].to_vec());
            p += n;
        }
        Ok(strings)
    }

    /// SPEC.md section 4.2.
    pub fn with_fallbacks(language: usize, files: &[Option<Vec<Vec<u8>>>]) -> Vec<Vec<u8>> {
        (0..COUNT)
            .map(|i| {
                [language, 0, 1]
                    .iter()
                    .filter_map(|&lang| files.get(lang).and_then(Option::as_ref))
                    .filter_map(|strings| strings.get(i))
                    .find(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    }
}

pub mod archive {
    use super::compress::DecompressError;
    use super::FormatError;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Entry {
        pub path: String,
        pub offset: u32,
        pub stored_size: u32,
        pub size: u32,
        pub flags: u32,
        pub checksum: u32,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum ReadError {
        Range,
        Scrambled,
        Decompress(DecompressError),
    }

    pub struct Archive<'a> {
        pub entries: Vec<Entry>,
        _data: &'a [u8],
    }

    impl<'a> Archive<'a> {
        /// SPEC.md section 5.2.
        pub fn parse(data: &'a [u8]) -> Result<Archive<'a>, FormatError> {
            fn word(data: &[u8], at: usize) -> u32 {
                u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
            }
            fn pieces(blob: &[u8], limit: u32) -> Vec<String> {
                blob.split(|&b| b == 0)
                    .take(limit as usize)
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect()
            }
            if data.len() < 4 {
                return Err(FormatError::Truncated { offset: 0 });
            }
            let dir = word(data, data.len() - 4) as usize;
            let header_end = dir.checked_add(24).filter(|&end| end <= data.len())
                .ok_or(FormatError::Truncated { offset: dir })?;
            let version = word(data, dir + 4);
            if version != 0x1000 {
                return Err(FormatError::BadMagic(version));
            }
            let folder_size = word(data, dir + 8) as usize;
            let folder_count = word(data, dir + 12);
            let name_size = word(data, dir + 16) as usize;
            let file_count = word(data, dir + 20);
            let folder_end = header_end.checked_add(folder_size)
                .filter(|&end| end <= data.len())
                .ok_or(FormatError::Truncated { offset: header_end })?;
            let name_end = folder_end.checked_add(name_size)
                .filter(|&end| end <= data.len())
                .ok_or(FormatError::Truncated { offset: folder_end })?;
            let table_size = (file_count as usize).checked_mul(24);
            let table_end = table_size.and_then(|n| name_end.checked_add(n))
                .filter(|&end| end <= data.len())
                .ok_or(FormatError::Truncated { offset: name_end })?;
            let folders = pieces(&data[header_end..folder_end], folder_count);
            let names = pieces(&data[folder_end..name_end], file_count);
            let mut entries = Vec::with_capacity(names.len().min(file_count as usize));
            for (j, name) in names.iter().enumerate().take((table_end - name_end) / 24) {
                let row = name_end + j * 24;
                let folder = i16::from_le_bytes(data[row + 12..row + 14].try_into().unwrap());
                let path = if folder >= 0 && (folder as usize) < folders.len() {
                    format!("{}\\{}", folders[folder as usize], name)
                } else {
                    name.clone()
                };
                entries.push(Entry {
                    path,
                    offset: word(data, row),
                    stored_size: word(data, row + 4),
                    size: word(data, row + 8),
                    flags: word(data, row + 16),
                    checksum: word(data, row + 20),
                });
            }
            Ok(Archive { entries, _data: data })
        }

        /// SPEC.md section 5.3.
        pub fn find(&self, path: &str) -> Option<&Entry> {
            let want = path.replace('/', "\\");
            self.entries.iter().find(|entry| {
                entry.path.len() == want.len()
                    && entry.path.bytes().zip(want.bytes()).all(|(a, b)| {
                        if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
                            a.eq_ignore_ascii_case(&b)
                        } else {
                            a == b
                        }
                    })
            })
        }

        /// SPEC.md section 5.4.
        pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, ReadError> {
            let start = entry.offset as usize;
            let length = entry.stored_size as usize;
            let end = start.checked_add(length).filter(|&end| end <= self._data.len())
                .ok_or(ReadError::Range)?;
            if entry.flags & 2 != 0 {
                return Err(ReadError::Scrambled);
            }
            let raw = &self._data[start..end];
            if entry.flags & 1 != 0 {
                super::compress::decompress(raw, entry.size as usize)
                    .map_err(ReadError::Decompress)
            } else {
                Ok(raw.to_vec())
            }
        }
    }
}


// ---------------------------------------------------------------------------------------------
// Batch 3 (SPEC.md sections 6 to 8). Stubs: implement the bodies, keep the public interface.

pub mod palette {
    use super::format::FileHeader;
    use super::FormatError;

    /// How a 16-bit palette entry is laid out.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PixelFormat {
        Argb4444,
        Rgb555,
        Rgb565,
    }

    /// SPEC.md section 6.1.
    pub fn format_from_extension(ext: &str) -> Option<PixelFormat> {
        match ext.chars().last()? {
            '4' => Some(PixelFormat::Argb4444),
            '5' => Some(PixelFormat::Rgb555),
            '6' => Some(PixelFormat::Rgb565),
            _ => None,
        }
    }

    /// SPEC.md section 6.2: 8-bit red, green, blue, alpha.
    pub fn to_rgba(format: PixelFormat, color: u16) -> [u8; 4] {
        fn scale(v: u32, bits: u32) -> u8 {
            let max = (1 << bits) - 1;
            ((v * 255 + max / 2) / max) as u8
        }
        match format {
            PixelFormat::Rgb565 => [scale((color >> 11) as u32, 5), scale(((color >> 5) & 63) as u32, 6), scale((color & 31) as u32, 5), 255],
            PixelFormat::Rgb555 => [scale(((color >> 10) & 31) as u32, 5), scale(((color >> 5) & 31) as u32, 5), scale((color & 31) as u32, 5), 255],
            PixelFormat::Argb4444 => [scale(((color >> 8) & 15) as u32, 4), scale(((color >> 4) & 15) as u32, 4), scale((color & 15) as u32, 4), scale((color >> 12) as u32, 4)],
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PaletteFile {
        pub header: FileHeader,
        pub format: PixelFormat,
        pub colors: Vec<u16>,
    }

    /// `at` could not give the requested colours; `len` is the number of colours from the start
    /// position to the end of the palette (SPEC.md section 6.4).
    #[derive(Debug, PartialEq, Eq)]
    pub struct BadPalette {
        pub len: usize,
    }

    /// SPEC.md section 6.3.
    pub fn parse_palette(data: &[u8], format: PixelFormat) -> Result<PaletteFile, FormatError> {
        let header = super::format::parse_header(data)?;
        let colors = data[20..].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        Ok(PaletteFile { header, format, colors })
    }

    impl PaletteFile {
        /// SPEC.md section 6.4.
        pub fn at(&self, offset: u32, len: usize) -> Result<&[u16], BadPalette> {
            if offset < 20 { return Err(BadPalette { len: 0 }); }
            let start = ((offset - 20) / 2) as usize;
            let remaining = self.colors.len().saturating_sub(start);
            if start.checked_add(len).map_or(true, |end| end > self.colors.len()) { return Err(BadPalette { len: remaining }); }
            Ok(&self.colors[start..start + len])
        }
    }
}

pub mod png {
    /// SPEC.md section 7.
    pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes {
                c ^= b as u32;
                for _ in 0..8 { c = if c & 1 != 0 { (c >> 1) ^ 0xedb88320 } else { c >> 1 }; }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind); out.extend_from_slice(data);
            let start = out.len() - data.len() - 4;
            out.extend_from_slice(&crc(&out[start..]).to_be_bytes());
        }
        let stride = width as usize * 4;
        let mut raw = Vec::with_capacity((stride + 1) * height as usize);
        for row in rgba.chunks_exact(stride) { raw.push(0); raw.extend_from_slice(row); }
        let mut z = vec![0x78, 0x01];
        for (i, block) in raw.chunks(65535).enumerate() {
            z.push(if (i + 1) * 65535 >= raw.len() { 1 } else { 0 });
            let n = block.len() as u16; z.extend_from_slice(&n.to_le_bytes()); z.extend_from_slice(&(!n).to_le_bytes()); z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13); ihdr.extend_from_slice(&width.to_be_bytes()); ihdr.extend_from_slice(&height.to_be_bytes()); ihdr.extend_from_slice(&[8,6,0,0,0]);
        chunk(&mut out, b"IHDR", &ihdr); chunk(&mut out, b"IDAT", &z); chunk(&mut out, b"IEND", &[]); out
    }
}

pub mod sound {
    use super::FormatError;

    /// Decoded audio: interleaved 16-bit samples.
    #[derive(Debug, PartialEq, Eq)]
    pub struct Pcm {
        pub channels: u16,
        pub rate: u32,
        pub samples: Vec<i16>,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum WavError {
        NotWav,
        /// The format tag that is not supported.
        Unsupported(u16),
        Truncated,
    }

    /// SPEC.md section 8.1.
    pub fn decode_wav(wav: &[u8]) -> Result<Pcm, WavError> {
        if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" { return Err(WavError::NotWav); }
        let mut fmt: Option<&[u8]> = None;
        let mut data: Option<&[u8]> = None;
        let mut p = 12usize;
        while wav.len() - p >= 8 {
            let id = &wav[p..p+4]; let n = u32::from_le_bytes(wav[p+4..p+8].try_into().unwrap()) as usize;
            let start = p + 8; let body = &wav[start..start.saturating_add(n).min(wav.len())];
            if id == b"fmt " { fmt = Some(body); } else if id == b"data" { data = Some(body); }
            p = match start.checked_add(n).and_then(|v| v.checked_add(n & 1)) { Some(v) => v, None => break };
            if p > wav.len() { break; }
        }
        let fmt = fmt.ok_or(WavError::Truncated)?; let data = data.ok_or(WavError::Truncated)?;
        if fmt.len() < 16 { return Err(WavError::Truncated); }
        let tag = u16::from_le_bytes([fmt[0],fmt[1]]); let channels = u16::from_le_bytes([fmt[2],fmt[3]]);
        let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
        let align = u16::from_le_bytes([fmt[12],fmt[13]]) as usize;
        let samples = if tag == 1 {
            data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],b[1]])).collect()
        } else if tag == 0x11 {
            if channels == 0 || align < 4 * channels as usize { return Err(WavError::Truncated); }
            const STEP: [i32; 89] = [7,8,9,10,11,12,13,14,16,17,19,21,23,25,28,31,34,37,41,45,50,55,60,66,73,80,88,97,107,118,130,143,157,173,190,209,230,253,279,307,337,371,408,449,494,544,598,658,724,796,876,963,1060,1166,1282,1411,1552,1707,1878,2066,2272,2499,2749,3024,3327,3660,4026,4428,4871,5358,5894,6484,7132,7845,8630,9493,10442,11487,12635,13899,15289,16818,18500,20350,22385,24623,27086,29794,32767];
            const IDX: [i32;16] = [-1,-1,-1,-1,2,4,6,8,-1,-1,-1,-1,2,4,6,8];
            let ch = channels as usize; let mut out = Vec::new();
            for block in data.chunks(align) {
                if block.len() < 4 * ch { break; }
                let mut pred = Vec::with_capacity(ch); let mut ix = Vec::with_capacity(ch);
                for c in 0..ch { pred.push(i16::from_le_bytes([block[4*c],block[4*c+1]]) as i32); ix.push((block[4*c+2] as usize).min(88)); }
                out.extend(pred.iter().map(|&v| v as i16));
                let body = &block[4*ch..];
                for group in body.chunks_exact(4*ch) {
                    for k in 0..8 { for c in 0..ch {
                        let byte = group[c*4 + k/2]; let nib = if k%2 == 0 { byte & 15 } else { byte >> 4 };
                        let step = STEP[ix[c]]; let diff = ((2 * (nib as i32 & 7) + 1) * step) >> 3;
                        pred[c] = (pred[c] + if nib & 8 != 0 { -diff } else { diff }).clamp(-32768,32767);
                        ix[c] = (ix[c] as i32 + IDX[nib as usize]).clamp(0,88) as usize;
                        out.push(pred[c] as i16);
                    }}
                }
            }
            out
        } else { return Err(WavError::Unsupported(tag)); };
        Ok(Pcm { channels, rate, samples })
    }

    pub struct SoundBank<'a> {
        _data: &'a [u8],
        offsets: Vec<u32>,
    }

    impl<'a> SoundBank<'a> {
        /// SPEC.md section 8.2.
        pub fn parse(snd: &'a [u8], sil: &[u8]) -> Result<SoundBank<'a>, FormatError> {
            super::format::parse_header(snd)?;
            let index = super::format::parse_index(sil)?;
            Ok(SoundBank { _data: snd, offsets: index.offsets })
        }

        pub fn len(&self) -> usize {
            self.offsets.len()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }

        /// SPEC.md section 8.2.
        pub fn wav(&self, n: usize) -> Result<Option<&'a [u8]>, FormatError> {
            let Some(&offset) = self.offsets.get(n) else { return Ok(None); };
            if offset == 0 { return Ok(None); }
            let start = offset as usize;
            let Some(head_end) = start.checked_add(8).filter(|&e| e <= self._data.len()) else {
                return Err(FormatError::BadOffset { index: n, offset });
            };
            let length = u32::from_le_bytes(self._data[start + 4..head_end].try_into().unwrap()) as usize;
            let end = head_end.checked_add(length).filter(|&e| e <= self._data.len()).ok_or(FormatError::BadOffset { index: n, offset })?;
            Ok(Some(&self._data[head_end..end]))
        }
    }
}

// Batch 4 (SPEC.md sections 9 and 10). Stubs: implement the bodies, keep the public interface.

pub mod dialogs {
    use super::FormatError;

    /// Size of one control record in bytes.
    pub const CONTROL_SIZE: usize = 36;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Control<'a> {
        pub id: u16,
        pub kind: u8,
        pub text_slot: i8,
        pub flags: u8,
        /// All 36 bytes of the record.
        pub raw: &'a [u8],
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Dialog<'a> {
        pub surface: u16,
        pub x: u16,
        pub y: u16,
        pub width: u16,
        pub height: u16,
        pub background: u16,
        pub flags: u16,
        pub controls: Vec<Control<'a>>,
    }

    /// SPEC.md section 9.
    pub fn parse(data: &[u8]) -> Result<Vec<Dialog<'_>>, FormatError> {
        if data.len() < 16 {
            return Err(FormatError::Truncated { offset: 0 });
        }
        let count = u32::from_le_bytes(data[4..8].try_into().unwrap()) as u64;
        let mut dialogs = Vec::new();
        for i in 0..count {
            let table_pos = 16u64 + 4 * i;
            if table_pos + 4 > data.len() as u64 {
                return Err(FormatError::Truncated { offset: table_pos as usize });
            }
            let table_pos = table_pos as usize;
            let offset = u32::from_le_bytes(data[table_pos..table_pos + 4].try_into().unwrap());
            let o = offset as u64;
            if o + 16 > data.len() as u64 {
                return Err(FormatError::BadOffset { index: i as usize, offset });
            }
            let o = o as usize;
            let u16at = |p: usize| u16::from_le_bytes(data[p..p + 2].try_into().unwrap());
            let n = u16at(o + 12) as u64;
            let controls_start = o as u64 + 16;
            let controls_end = controls_start + n * CONTROL_SIZE as u64;
            if controls_end > data.len() as u64 {
                return Err(FormatError::Truncated { offset: controls_start as usize });
            }
            let mut controls = Vec::with_capacity(n as usize);
            for j in 0..n as usize {
                let start = o + 16 + j * CONTROL_SIZE;
                let raw = &data[start..start + CONTROL_SIZE];
                controls.push(Control {
                    id: u16::from_le_bytes(raw[0x0a..0x0c].try_into().unwrap()),
                    kind: raw[0x18],
                    text_slot: raw[0x19] as i8,
                    flags: raw[0x1b],
                    raw,
                });
            }
            dialogs.push(Dialog {
                surface: u16at(o), x: u16at(o + 2), y: u16at(o + 4),
                width: u16at(o + 6), height: u16at(o + 8), background: u16at(o + 10),
                flags: u16at(o + 14), controls,
            });
        }
        Ok(dialogs)
    }
}

pub mod landscape {
    use super::format::FileHeader;
    use super::FormatError;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Record<'a> {
        pub flags: u16,
        pub pages: u8,
        pub data: &'a [u8],
    }

    impl Record<'_> {
        /// SPEC.md section 10.2.
        pub fn is_software(&self) -> bool {
            self.flags & 2 != 0
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct LandscapeFile<'a> {
        pub header: FileHeader,
        pub records: Vec<Record<'a>>,
    }

    impl<'a> LandscapeFile<'a> {
        /// SPEC.md section 10.1.
        pub fn parse(data: &'a [u8]) -> Result<LandscapeFile<'a>, FormatError> {
            let header = super::format::parse_header(data)?;
            let mut records = Vec::new();
            let mut p = 20usize;
            while (p as u64) + 8 <= data.len() as u64 {
                let flags = u16::from_le_bytes(data[p..p + 2].try_into().unwrap());
                let pages = data[p + 3];
                let size = u32::from_le_bytes(data[p + 4..p + 8].try_into().unwrap()) as u64;
                let start = p + 8;
                if size == 0 {
                    records.push(Record { flags, pages, data: &data[start..] });
                    break;
                }
                let end = start as u64 + size;
                if end > data.len() as u64 {
                    return Err(FormatError::Truncated { offset: start });
                }
                records.push(Record { flags, pages, data: &data[start..end as usize] });
                p = end as usize;
            }
            Ok(LandscapeFile { header, records })
        }

        /// SPEC.md section 10.2: `(side length in pixels, one list of side*side pixels per page)`.
        pub fn hardware_pages(&self) -> Option<(usize, Vec<Vec<u16>>)> {
            let record = self.records.iter().find(|r| !r.is_software() && r.pages > 0)?;
            let page_bytes = record.data.len() / record.pages as usize;
            if page_bytes == 0 { return None; }
            let half = page_bytes / 2;
            let mut lo = 0usize;
            let mut hi = half.saturating_add(1);
            while lo + 1 < hi {
                let mid = lo + (hi - lo) / 2;
                if mid <= half / mid.max(1) { lo = mid; } else { hi = mid; }
            }
            let side = lo;
            if side * side * 2 != page_bytes { return None; }
            let mut pages = Vec::with_capacity(record.data.len() / page_bytes);
            for bytes in record.data.chunks_exact(page_bytes) {
                pages.push(bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect());
            }
            Some((side, pages))
        }
    }
}

// Batch 5 (SPEC.md section 11). Stubs: implement the bodies, keep the public interface.

pub mod map {
    use super::compress::DecompressError;
    use super::FormatError;

    /// Chunk kinds (the low 16 bits of a chunk's type word) that the accessors below use.
    pub const PREVIEW: u16 = 4;
    pub const OBJECTS: u16 = 6;
    pub const LANDSCAPE: u16 = 13;
    pub const SETTLERS: u16 = 7;
    pub const BUILDINGS: u16 = 8;
    pub const PILES: u16 = 9;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ChunkHeader {
        /// The whole type word.
        pub kind: u32,
        pub stored: u32,
        pub size: u32,
        pub checksum: u32,
        pub param: u32,
    }

    impl ChunkHeader {
        /// SPEC.md section 11.3: the low 16 bits of the type word.
        pub fn kind(&self) -> u16 {
            self.kind as u16
        }

        /// SPEC.md section 11.3: the high 16 bits of the type word.
        pub fn index(&self) -> u16 {
            (self.kind >> 16) as u16
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Chunk<'a> {
        pub header: ChunkHeader,
        pub stored: &'a [u8],
    }

    impl Chunk<'_> {
        /// SPEC.md section 11.3: the decompressed data (`header.size` bytes).
        pub fn data(&self) -> Result<Vec<u8>, DecompressError> {
            super::compress::decompress(self.stored, self.header.size as usize)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct MapFile<'a> {
        pub checksum: u32,
        pub version: u32,
        pub chunks: Vec<Chunk<'a>>,
    }

    /// A settler or vehicle placed on the map (SPEC.md section 11.4).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Settler {
        pub x: u16,
        pub y: u16,
        pub kind: u8,
        pub player: u8,
    }

    /// A building placed on the map (SPEC.md section 11.4).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Building {
        pub x: u16,
        pub y: u16,
        pub kind: u8,
        pub player: u8,
        pub occupied: u8,
        pub soldiers: [u8; 9],
    }

    /// A pile of goods (SPEC.md section 11.4).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Pile {
        pub x: u16,
        pub y: u16,
        pub good: u8,
        pub amount: u8,
        pub flag: u8,
    }

    impl<'a> MapFile<'a> {
        /// SPEC.md section 11.2.
        pub fn parse(data: &'a [u8]) -> Result<MapFile<'a>, FormatError> {
            if data.len() < 8 { return Err(FormatError::Truncated { offset: 0 }); }
            let checksum = u32::from_le_bytes(data[0..4].try_into().unwrap());
            let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
            if version != 0x28 && version != 0x1f { return Err(FormatError::BadMagic(version)); }
            let mut chunks = Vec::new();
            let mut p = 8usize;
            while p.checked_add(24).map_or(false, |end| end <= data.len()) {
                let mut raw = data[p..p + 24].to_vec();
                super::cipher::apply_default(&mut raw);
                let word = |i: usize| u32::from_le_bytes(raw[i..i + 4].try_into().unwrap());
                let header = ChunkHeader { kind: word(0), stored: word(4), size: word(8), checksum: word(12), param: word(20) };
                let start = p + 24;
                let end = (start as u64) + (header.stored as u64);
                if end > data.len() as u64 { return Err(FormatError::Truncated { offset: start }); }
                chunks.push(Chunk { header, stored: &data[start..end as usize] });
                if header.kind() == 0 || header.kind() == 5 { break; }
                p = end as usize;
            }
            Ok(MapFile { checksum, version, chunks })
        }

        /// SPEC.md section 11.3: the first chunk whose `kind()` is `kind`.
        pub fn chunk(&self, kind: u16) -> Option<&Chunk<'a>> {
            self.chunks.iter().find(|c| c.header.kind() == kind)
        }

        /// SPEC.md section 11.3: the side length of the (square) map.
        pub fn size(&self) -> Option<u32> {
            self.chunk(LANDSCAPE).map(|c| c.header.param)
        }

        /// SPEC.md section 11.4.
        pub fn settlers(&self) -> Vec<Settler> {
            self.chunk(SETTLERS).and_then(|c| c.data().ok()).map(|d| d.chunks_exact(6).map(|r| Settler { x: u16::from_le_bytes([r[0],r[1]]), y: u16::from_le_bytes([r[2],r[3]]), kind:r[4], player:r[5] }).collect()).unwrap_or_default()
        }

        /// SPEC.md section 11.4.
        pub fn buildings(&self) -> Vec<Building> {
            self.chunk(BUILDINGS).and_then(|c| c.data().ok()).map(|d| d.chunks_exact(20).map(|r| Building { x:u16::from_le_bytes([r[0],r[1]]), y:u16::from_le_bytes([r[2],r[3]]), kind:r[4], player:r[5], occupied:r[6], soldiers:r[7..16].try_into().unwrap() }).collect()).unwrap_or_default()
        }

        /// SPEC.md section 11.4.
        pub fn piles(&self) -> Vec<Pile> {
            self.chunk(PILES).and_then(|c| c.data().ok()).map(|d| d.chunks_exact(8).map(|r| Pile { x:u16::from_le_bytes([r[0],r[1]]), y:u16::from_le_bytes([r[2],r[3]]), good:r[4], amount:r[5], flag:r[6] }).collect()).unwrap_or_default()
        }
    }

    /// SPEC.md section 11.5: the checksum of a whole map file (computed over the bytes after the first 8).
    pub fn checksum(file: &[u8]) -> u32 {
        crc16_swapped(file.get(8..).unwrap_or(&[]))
    }

    /// SPEC.md section 11.5.
    pub fn crc16_swapped(data: &[u8]) -> u32 {
        let mut crc = 0u16;
        for &b in data {
            crc ^= b as u16;
            for _ in 0..8 { crc = if crc & 1 != 0 { (crc >> 1) ^ 0xa001 } else { crc >> 1 }; }
        }
        crc.swap_bytes() as u32
    }
}

// Batch 6 (SPEC.md sections 12 and 13). Stubs: implement the bodies, keep the public interface.

pub mod sprites {
    use super::format::{FileHeader, Index};
    use super::FormatError;

    /// The raw-pixels flag of a long-form image header.
    pub const FLAG_RAW: u16 = 0x20;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ImageHeader {
        pub width: u16,
        pub height: u16,
        pub x: i16,
        pub y: i16,
        pub flags: u16,
        pub extra: u16,
        /// 8 (short form) or 12 (long form).
        pub size: u8,
    }

    impl ImageHeader {
        /// SPEC.md section 12.2.
        pub fn is_raw(&self) -> bool {
            self.size == 12 && self.flags & FLAG_RAW != 0
        }

        /// SPEC.md section 12.1.
        pub fn parse(data: &[u8], at: usize) -> Result<ImageHeader, FormatError> {
            let end2 = at.checked_add(2).ok_or(FormatError::Truncated { offset: at })?;
            if end2 > data.len() {
                return Err(FormatError::Truncated { offset: at });
            }
            let u16_at = |p: usize| u16::from_le_bytes([data[p], data[p + 1]]);
            let w0 = u16_at(at);
            if w0 >= 861 {
                let end = at.checked_add(8).ok_or(FormatError::Truncated { offset: at })?;
                if end > data.len() {
                    return Err(FormatError::Truncated { offset: at });
                }
                Ok(ImageHeader {
                    width: data[at] as u16,
                    height: data[at + 1] as u16,
                    x: data[at + 2] as i8 as i16,
                    y: data[at + 3] as i8 as i16,
                    flags: u16_at(at + 4),
                    extra: u16_at(at + 6),
                    size: 8,
                })
            } else {
                let end = at.checked_add(12).ok_or(FormatError::Truncated { offset: at })?;
                if end > data.len() {
                    return Err(FormatError::Truncated { offset: at });
                }
                Ok(ImageHeader {
                    width: w0,
                    height: u16_at(at + 2),
                    x: u16_at(at + 4) as i16,
                    y: u16_at(at + 6) as i16,
                    flags: u16_at(at + 8),
                    extra: u16_at(at + 10),
                    size: 12,
                })
            }
        }
    }

    /// One encoded image inside a sprite file.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Image<'a> {
        pub header: ImageHeader,
        /// Position of the image header in the file.
        pub offset: usize,
        /// The encoded pixels (for run-length images up to and including the end marker).
        pub data: &'a [u8],
    }

    /// A decoded pixel.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Pixel {
        Transparent,
        Shadow,
        Index(u8),
    }

    impl<'a> Image<'a> {
        /// SPEC.md section 12.3.
        pub fn at(data: &'a [u8], offset: usize) -> Result<Image<'a>, FormatError> {
            let header = ImageHeader::parse(data, offset)?;
            let start = offset.checked_add(header.size as usize)
                .ok_or(FormatError::Truncated { offset })?;
            if header.is_raw() {
                let len = header.width as usize * header.height as usize;
                let end = start.checked_add(len).ok_or(FormatError::Truncated { offset: start })?;
                if end > data.len() {
                    return Err(FormatError::Truncated { offset: start });
                }
                return Ok(Image { header, offset, data: &data[start..end] });
            }
            let mut p = start;
            loop {
                if p >= data.len() {
                    return Err(FormatError::Truncated { offset: p });
                }
                let b = data[p];
                p += 1;
                if b < 2 {
                    if p >= data.len() {
                        return Err(FormatError::Truncated { offset: p });
                    }
                    let n = data[p];
                    p += 1;
                    if n == 0 {
                        break;
                    }
                }
            }
            Ok(Image { header, offset, data: &data[start..p] })
        }

        /// SPEC.md section 12.3: the position just after the image.
        pub fn end(&self) -> usize {
            self.offset + self.header.size as usize + self.data.len()
        }

        /// SPEC.md section 12.4.
        pub fn pixels(&self) -> Vec<Pixel> {
            let total = self.header.width as usize * self.header.height as usize;
            let mut pixels = Vec::with_capacity(total);
            if self.header.is_raw() {
                pixels.extend(self.data.iter().take(total).map(|&b| {
                    if b == 0 { Pixel::Transparent } else { Pixel::Index(b) }
                }));
            } else {
                let mut p = 0;
                while pixels.len() < total && p < self.data.len() {
                    let b = self.data[p];
                    p += 1;
                    if b >= 2 {
                        pixels.push(Pixel::Index(b));
                    } else {
                        let n = if p < self.data.len() { self.data[p] } else { 0 };
                        p += usize::from(p < self.data.len());
                        if n == 0 { break; }
                        let count = (n as usize).min(total - pixels.len());
                        pixels.extend(std::iter::repeat(if b == 0 { Pixel::Transparent } else { Pixel::Shadow }).take(count));
                    }
                }
            }
            pixels.resize(total, Pixel::Transparent);
            pixels
        }
    }

    /// A sprite file.
    pub struct GfxFile<'a> {
        pub header: FileHeader,
        _data: &'a [u8],
    }

    impl<'a> GfxFile<'a> {
        /// SPEC.md section 12.5.
        pub fn parse(data: &'a [u8]) -> Result<GfxFile<'a>, FormatError> {
            let header = super::format::parse_header(data)?;
            Ok(GfxFile { header, _data: data })
        }

        /// SPEC.md section 12.5.
        pub fn image_at(&self, offset: u32) -> Result<Image<'a>, FormatError> {
            Image::at(self._data, offset as usize)
        }

        /// SPEC.md section 12.5.
        pub fn images(&self) -> impl Iterator<Item = Result<Image<'a>, FormatError>> + '_ {
            let mut position = 20usize;
            std::iter::from_fn(move || {
                if position >= self._data.len() { return None; }
                match Image::at(self._data, position) {
                    Ok(image) => { position = image.end(); Some(Ok(image)) }
                    Err(error) => { position = self._data.len(); Some(Err(error)) }
                }
            })
        }

        /// SPEC.md section 12.5.
        pub fn indexed(&self, index: &Index) -> Result<Vec<Option<Image<'a>>>, FormatError> {
            let mut result = Vec::with_capacity(index.offsets.len());
            for (i, &offset) in index.offsets.iter().enumerate() {
                if offset == 0 { result.push(None); }
                else if offset as usize >= self._data.len() {
                    return Err(FormatError::BadOffset { index: i, offset });
                } else {
                    result.push(Some(self.image_at(offset)?));
                }
            }
            Ok(result)
        }
    }

    /// SPEC.md section 13: `result[job][direction]` is a list of entry numbers of the image index.
    pub fn job_frames(jobs: &Index, directions: &Index, image_count: usize) -> Vec<Vec<Vec<usize>>> {
        let starts = {
            let mut v: Vec<usize> = directions.offsets.iter()
                .filter_map(|&o| super::format::entry_number(o)).collect();
            v.push(image_count);
            v.sort_unstable();
            v.dedup();
            v
        };
        let frames_from = |first: usize| -> Vec<usize> {
            let next = starts.iter().copied().find(|&v| v > first).unwrap_or(image_count);
            if next <= first { Vec::new() } else { (first..next).collect() }
        };
        let direction_count = directions.offsets.len();
        let mut job_starts: Vec<usize> = jobs.offsets.iter()
            .filter_map(|&o| super::format::entry_number(o)).collect();
        job_starts.push(direction_count);
        jobs.offsets.iter().map(|&o| {
            let Some(first) = super::format::entry_number(o) else { return Vec::new(); };
            let next = job_starts.iter().copied().filter(|&v| v > first).min().unwrap_or(direction_count);
            let end = next.min(direction_count);
            if end <= first { return Vec::new(); }
            (first..end).map(|d| {
                let q = directions.offsets[d];
                if q == 0 { Vec::new() }
                else if let Some(frame) = super::format::entry_number(q) { frames_from(frame) }
                else { Vec::new() }
            }).collect()
        }).collect()
    }
}

pub mod terrain {
    pub const TERRAIN_COLORS: [[u8; 3]; 256] = [
        [0, 85, 110],
        [0, 76, 101],
        [0, 67, 92],
        [0, 58, 83],
        [0, 49, 74],
        [0, 40, 65],
        [0, 31, 54],
        [0, 22, 47],
        [100, 90, 50],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 100, 0],
        [10, 80, 10],
        [30, 100, 70],
        [0, 0, 0],
        [50, 80, 0],
        [30, 80, 0],
        [0, 0, 0],
        [20, 70, 40],
        [40, 80, 20],
        [60, 100, 30],
        [0, 0, 0],
        [0, 0, 0],
        [98, 90, 90],
        [70, 60, 60],
        [0, 0, 0],
        [0, 0, 0],
        [40, 40, 30],
        [20, 60, 20],
        [0, 0, 0],
        [65, 70, 65],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [100, 90, 50],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [100, 70, 0],
        [80, 70, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [60, 0, 0],
        [40, 50, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [50, 70, 100],
        [50, 80, 100],
        [60, 70, 100],
        [50, 50, 100],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [88, 92, 98],
        [76, 80, 82],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [40, 25, 55],
        [30, 45, 50],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
    ];
    pub const TEXTURE_CLASS: [u8; 256] = [
        0, 1, 2, 3, 4, 5, 6, 7, 48, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
        48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
        64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79,
        80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
        96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,
        112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127,
        128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143,
        144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159,
        160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175,
        176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191,
        192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207,
        208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,
        224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239,
        240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
    ];
    pub const CLASS_PAGE: [i8; 256] = [
        -1i8, -1i8, -1i8, -1i8, -1i8, -1i8, -1i8, 4i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, -1i8, 1i8, 0i8, -1i8, -1i8, 0i8, -1i8, 2i8, -1i8, 0i8, 0i8, 20i8, 21i8, 0i8, 0i8,
        8i8, -1i8, 0i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        7i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        10i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        15i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        -1i8, -1i8, -1i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        17i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        13i8, -1i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8,
        0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 0i8, 3i8, 0i8, 0i8, 0i8, 0i8, 0i8,
    ];
    pub const TRANSITION_INDEX: [u8; 256] = [
        9, 10, 11, 12, 13, 14, 15, 16, 29, 0, 0, 0, 0, 0, 0, 0,
        0, 19, 32, 0, 17, 18, 0, 22, 30, 31, 0, 0, 33, 34, 0, 0,
        3, 25, 0, 21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        1, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        2, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        35, 36, 37, 38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        4, 26, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        5, 27, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        7, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    pub const TEXELS_PER_TILE: u32 = 32;
    pub const TRANSITIONS: &[(u8, u8, [u8; 4])] = &[
        (0, 6, [3, 0, 3, 129]),
        (0, 9, [3, 0, 18, 129]),
        (0, 17, [2, 2, 11, 129]),
        (0, 18, [2, 0, 14, 129]),
        (0, 19, [2, 0, 9, 129]),
        (0, 22, [2, 2, 12, 129]),
        (0, 31, [1, 0, 3, 129]),
        (0, 32, [3, 2, 3, 129]),
        (0, 33, [1, 0, 19, 129]),
        (0, 34, [3, 0, 19, 129]),
        (0, 35, [0, 0, 18, 129]),
        (0, 36, [1, 0, 18, 129]),
        (0, 37, [2, 0, 18, 129]),
        (0, 38, [3, 0, 18, 129]),
        (1, 23, [3, 0, 11, 129]),
        (2, 24, [3, 2, 14, 129]),
        (3, 21, [2, 0, 16, 129]),
        (3, 25, [3, 2, 9, 129]),
        (5, 27, [3, 2, 16, 129]),
        (6, 0, [2, 0, 3, 129]),
        (6, 9, [0, 2, 5, 129]),
        (7, 28, [3, 0, 12, 129]),
        (9, 0, [3, 2, 18, 129]),
        (9, 6, [1, 2, 5, 129]),
        (9, 9, [0, 0, 5, 128]),
        (9, 10, [0, 0, 6, 128]),
        (9, 38, [0, 0, 5, 128]),
        (10, 9, [0, 1, 6, 128]),
        (10, 10, [1, 0, 5, 128]),
        (10, 11, [1, 0, 6, 128]),
        (11, 10, [1, 1, 6, 128]),
        (11, 11, [2, 0, 5, 128]),
        (11, 12, [2, 0, 6, 128]),
        (12, 11, [2, 1, 6, 128]),
        (12, 12, [3, 0, 5, 128]),
        (12, 13, [3, 0, 6, 128]),
        (13, 12, [3, 1, 6, 128]),
        (13, 13, [0, 1, 5, 128]),
        (13, 14, [0, 2, 6, 128]),
        (14, 13, [0, 3, 6, 128]),
        (14, 14, [1, 1, 5, 128]),
        (14, 15, [1, 2, 6, 128]),
        (15, 14, [1, 3, 6, 128]),
        (15, 15, [2, 1, 5, 128]),
        (15, 16, [2, 2, 6, 128]),
        (16, 13, [2, 3, 5, 128]),
        (16, 14, [2, 2, 5, 128]),
        (16, 15, [2, 3, 6, 128]),
        (17, 0, [3, 2, 11, 129]),
        (17, 17, [0, 0, 11, 128]),
        (17, 23, [0, 2, 11, 129]),
        (18, 0, [3, 0, 14, 129]),
        (18, 18, [0, 0, 14, 128]),
        (18, 24, [0, 2, 14, 129]),
        (19, 0, [3, 0, 9, 129]),
        (19, 19, [0, 0, 9, 128]),
        (19, 25, [0, 2, 9, 129]),
        (21, 3, [3, 0, 16, 129]),
        (21, 21, [0, 0, 16, 128]),
        (21, 27, [0, 2, 16, 129]),
        (22, 0, [3, 2, 12, 129]),
        (22, 22, [0, 0, 12, 128]),
        (22, 28, [0, 2, 12, 129]),
        (23, 1, [2, 0, 11, 129]),
        (23, 17, [1, 2, 11, 129]),
        (23, 23, [1, 0, 11, 128]),
        (24, 2, [2, 2, 14, 129]),
        (24, 18, [1, 2, 14, 129]),
        (24, 24, [1, 0, 14, 128]),
        (25, 3, [2, 2, 9, 129]),
        (25, 19, [1, 2, 9, 129]),
        (25, 25, [1, 0, 9, 128]),
        (27, 5, [2, 2, 16, 129]),
        (27, 21, [1, 2, 16, 129]),
        (27, 27, [1, 0, 16, 128]),
        (28, 7, [2, 0, 12, 129]),
        (28, 22, [1, 2, 12, 129]),
        (28, 28, [1, 0, 12, 128]),
        (30, 31, [1, 2, 3, 129]),
        (31, 0, [0, 0, 3, 129]),
        (31, 30, [0, 2, 3, 129]),
        (31, 31, [0, 1, 9, 128]),
        (32, 0, [2, 2, 3, 129]),
        (33, 0, [0, 0, 19, 129]),
        (34, 0, [2, 0, 19, 129]),
        (35, 0, [0, 2, 18, 129]),
        (35, 35, [0, 0, 5, 128]),
        (35, 36, [0, 0, 5, 128]),
        (35, 37, [0, 0, 5, 128]),
        (35, 38, [0, 0, 5, 128]),
        (36, 0, [1, 2, 18, 129]),
        (36, 35, [0, 0, 5, 128]),
        (36, 36, [0, 0, 5, 128]),
        (36, 37, [0, 0, 5, 128]),
        (36, 38, [0, 0, 5, 128]),
        (37, 0, [2, 2, 18, 129]),
        (37, 35, [0, 0, 5, 128]),
        (37, 36, [0, 0, 5, 128]),
        (37, 37, [0, 0, 5, 128]),
        (37, 38, [0, 0, 5, 128]),
        (38, 0, [3, 2, 18, 129]),
        (38, 9, [0, 0, 5, 128]),
        (38, 35, [0, 0, 5, 128]),
        (38, 36, [0, 0, 5, 128]),
        (38, 37, [0, 0, 5, 128]),
        (38, 38, [0, 0, 5, 128]),
    ];
    /// SPEC.md section 14.2.
    pub fn transition(pair: u8, odd: u8) -> Option<[u8; 4]> {
        let a = TRANSITION_INDEX[pair as usize];
        let b = TRANSITION_INDEX[odd as usize];
        TRANSITIONS.iter().find(|&&(x, y, _)| x == a && y == b).map(|&(_, _, values)| values)
    }

    /// SPEC.md section 14.3: the minimap colour (red, green, blue) of a landscape tile of four bytes.
    pub fn minimap_color(tile: [u8; 4]) -> [u8; 3] {
        let color = TERRAIN_COLORS[tile[1] as usize];
        let shade = ((tile[2] & 0x0f) as i32) * 8 - 64;
        let mut c = [color[0] as i32 + shade, color[1] as i32 + shade, color[2] as i32 + shade];
        if tile[2] & 0x40 != 0 {
            let grey = (c[0] + c[1] + 2 * c[2]) / 4;
            c = [grey; 3];
        }
        let fog = (tile[3] >> 3) & 7;
        if fog == 7 { return [9; 3]; }
        if fog != 0 { for v in &mut c { *v /= 2; } }
        c.map(|v| v.clamp(9, 255) as u8)
    }
}

pub mod game_files {
    use std::path::{Path, PathBuf};

    /// The game folder and the archives mounted from it (SPEC.md section 15).
    pub struct GameFiles {
        root: PathBuf,
        archives: Vec<Vec<u8>>,
    }

    impl GameFiles {
        /// SPEC.md section 15.1.
        pub fn open(root: impl AsRef<Path>) -> std::io::Result<GameFiles> {
            let root = root.as_ref().to_path_buf();
            let mut archives = Vec::new();
            for name in ["game.lib", "gfx.lib"] {
                if let Ok(bytes) = std::fs::read(root.join(name)) {
                    if super::archive::Archive::parse(&bytes).is_ok() { archives.push(bytes); }
                }
            }
            Ok(GameFiles { root, archives })
        }

        /// SPEC.md section 15.2.
        pub fn read(&self, path: &str) -> Option<Vec<u8>> {
            let relative = path.replace('\\', "/");
            let loose = self.root.join(relative);
            if let Ok(metadata) = std::fs::metadata(&loose) {
                if metadata.is_file() {
                    if let Ok(data) = std::fs::read(loose) { return Some(data); }
                }
            }
            for bytes in &self.archives {
                if let Ok(archive) = super::archive::Archive::parse(bytes) {
                    if let Some(entry) = archive.find(path) {
                        if let Ok(data) = archive.read(entry) { return Some(data); }
                    }
                }
            }
            None
        }
    }
}
