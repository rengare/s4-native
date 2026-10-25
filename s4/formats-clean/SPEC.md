# Specification

Two byte-level primitives used by the data files of a legacy game. This document describes **observable
behaviour**: what goes in and what comes out. Where the text is ambiguous or incomplete, the vectors in
`vectors/` are authoritative (they were produced by running the reference implementation); where both are
silent, the behaviour is **unspecified** and tests do not cover it.

Conventions: numbers are unsigned 32-bit unless stated; `>>` is a logical right shift, `<<` a left shift
(bits shifted out are lost), `^` XOR, `|` OR, `&` AND, all on 32-bit values; hexadecimal is written `0x…`.
"Bit 0" is the least significant bit.

---

## 1. Key-stream cipher

A byte-oriented stream cipher. Encrypting and decrypting are the same operation: the data is XORed with a
key stream that depends only on the key.

### 1.1 Interface

- `KeystreamCipher::new(key)`: any byte string, including the empty one.
- `apply(data)`: XORs `data` in place with the next `data.len()` bytes of the key stream. The generator keeps
  its state between calls, so calling `apply` on consecutive slices gives the same result as one call on the
  whole.
- `DEFAULT_KEY` is the 20 ASCII bytes `01234567890123456789`; `apply_default(data)` uses a fresh cipher with it.

### 1.2 State and key set-up

The state is three 32-bit registers **A**, **B**, **C**.

1. *Padded key.* If `key` is empty, the padded key is the 12 ASCII bytes `Default Seed`. Otherwise, if it is
   shorter than 12 bytes, it is extended to 12 bytes by repeating itself cyclically (`key[0], key[1], …`
   after its last byte). A key of 12 bytes or more is used as it is; only its first 12 bytes matter.
2. *Registers.* Register A is built from padded-key bytes 0..=3, B from bytes 4..=7, C from bytes 8..=11. For
   each register, start with `r = 0` and for each of its four bytes in order: `r = (r << 8) | s(byte)`, where
   `s(byte)` is the byte **sign-extended to 32 bits** (a byte `0x00..=0x7f` stays as it is; a byte
   `0x80..=0xff` becomes `0xFFFFFF80..=0xFFFFFFFF`). Because the extension is OR-ed in, a byte with its top
   bit set sets all bits above it in `r`.
3. *Defaults.* A register that is 0 after step 2 is replaced by its default: A = `0x13579BDF`,
   B = `0x2468ACE0`, C = `0xFDB97531`.

### 1.3 Producing one key-stream byte

Let `ob` and `oc` be single bits; at the start of every byte set `ob = B & 1` and `oc = C & 1` (the current
lowest bits). Then repeat the following step **eight times**; after each step append the bit `ob ^ oc` to the
byte being built, most significant bit first (the first step gives bit 7, the last bit 0).

One step:

- If `A & 1 == 0`:
  - `A = A >> 1`.
  - Step C: if `C & 1 == 0` then `C = (C >> 1) & 0x0FFFFFFF` and `oc = 0`;
    otherwise `C = ((C ^ 0x10000002) >> 1) | 0xF0000000` and `oc = 1`.
- Otherwise (`A & 1 == 1`):
  - `A = ((A ^ 0x80000062) >> 1) | 0x80000000`.
  - Step B: if `B & 1 == 0` then `B = (B >> 1) & 0x3FFFFFFF` and `ob = 0`;
    otherwise `B = ((B ^ 0x40000020) >> 1) | 0xC0000000` and `ob = 1`.

(`ob` keeps its value when B is not stepped, `oc` when C is not stepped, within the same byte; both are
re-initialised at the start of the next byte as described above.)

### 1.4 Vectors

`vectors/cipher.tsv`: key, input, output in hex; many keys (empty, short, exactly 12 bytes, longer than 12,
bytes with the top bit set, keys that give a zero register) and the output for all-zero input, which is the key
stream itself. `vectors/data/long-*.keystream`: 70,000 bytes of key stream.

---

## 2. Adaptive-Huffman + LZ77 decompressor

Decompresses a bit stream into a byte string of a length that the caller already knows.

### 2.1 Interface

`decompress(src, expected_len) -> Result<Vec<u8>, DecompressError>` where the error is `Truncated` (the input
ended while a code was being read) or `Corrupt` (see 2.7). On success the result has exactly `expected_len`
bytes.

### 2.2 Bits

The input is read **most significant bit first**: the first bit read is bit 7 of `src[0]`, the eighth is bit 0
of `src[0]`, then bit 7 of `src[1]`, and so on. `read(n)` returns the next `n` bits as a number (the first bit
read is the most significant); `read(0)` is 0 and consumes nothing. If fewer than `n` bits remain,
`read(n)` fails and the decompression ends with `Truncated`. A byte counts as *fetched* as soon as any of its
bits has been read.

`unary()` counts how many 0 bits precede the next 1 bit and consumes that 1 bit as well. If the input ends
before a 1 bit is found, the count so far is returned and nothing fails.

### 2.3 Tables (decoder state)

All of these keep their values for the whole call, including across the "streams" of 2.6:

- `classes`: 16 pairs `(extra, first)`, initially:
  `(2,0) (3,4) (3,12) (4,20) (4,36) (4,52) (4,68) (4,84) (4,100) (4,116) (4,132) (4,148) (4,164) (5,180) (5,212) (5,244)`.
- `symbols`: a list of 274 symbol numbers (`0..=0x111`), initially, in this order:
  `0x100, 0x101, …, 0x10F` (16 values), then `0, 32, 48, 255`, then every value from `1` to `254` in
  ascending order **except 32 and 48** (252 values), then `0x110`, then `0x111`.
- `freq`: 274 signed 16-bit counters (one per symbol number), initially 0. Increments wrap around.
- `window`: 65,536 bytes, initially all 0, used as a ring: position `p` is stored at `p mod 65536`; and a
  counter `w` of the total number of bytes output so far (it starts at 0 and counts all output of the call).

### 2.4 Decoding one symbol

1. `c = read(4)`, `(extra, first) = classes[c]`, `i = first + read(extra)`.
2. If `i >= 274` the result is `Corrupt`. Otherwise `s = symbols[i]` and `freq[s]` is incremented.
3. Act on `s`:

**Literal, `s <= 0xFF`.** If the output already has `expected_len` bytes the result is `Corrupt`. Otherwise
the byte `s` is appended to the output and stored in the window at `w mod 65536`, and `w` grows by one.

**Match, `0x100 <= s <= 0x10F`.** Let `code = s - 0x100`.
- *Length:* if `code < 8`, `length = 4 + code`. Otherwise use this table indexed by `code - 8`, entries
  `(extra, base)`: `(1,8) (2,10) (3,14) (4,22) (5,38) (6,70) (7,134) (8,262)`, and
  `length = 4 + base + read(extra)`.
- *Distance:* `k = read(3)`, with table `(extra, base)`:
  `(0,0) (0,1) (1,2) (2,4) (3,8) (4,16) (5,32) (6,64)` indexed by `k`; then `high = read(8)`,
  `low = read(extra + 1)`, and `distance = ((high << (extra + 1)) | low) + base * 512`.
- All of these fields are read first (so running out of input gives `Truncated`). Then, if the output length
  plus `length` is more than `expected_len`, the result is `Corrupt`.
- Otherwise copy `length` bytes, **one at a time**: each byte is `window[(w - distance) mod 65536]` (so a copy
  may read bytes that it has just written); it is appended to the output and written to `window[w mod 65536]`,
  and `w` grows by one.

**Rebuild, `s = 0x110`.** The counter of this very symbol has already been incremented in step 2.
1. Form the 274 pairs `(freq[t], t)` for `t = 0..=273` from the counters as they are now.
2. Sort the pairs **in descending order**, comparing the signed counter first and the symbol number second
   (the larger counter first; among equal counters the larger symbol number first). The new `symbols` list is
   the symbol numbers in that order.
3. Halve every `freq` entry (arithmetic shift right by one, i.e. rounding towards minus infinity).
4. New `classes`: start with `width = 0`, `first = 0`; for `c = 0..=15`: `width = width + unary()`, then
   `classes[c] = (width, first)`, then `first = first + 2^width` (computed in 32 bits; a width of 32 or more
   contributes 0).

**End, `s = 0x111`.**
- If the output length equals `expected_len`, the call succeeds and returns the output; input that is still
  left over is ignored.
- Otherwise (the output is shorter): if the input has at least one byte that has not been fetched yet, the
  rest of the current byte is skipped, and decoding goes on with the next whole byte using **the same tables
  and window** (a new "stream"; see 2.6). If there is no such byte the result is `Corrupt`.

### 2.5 Main loop

Decode symbols (2.4) one after another until the call succeeds (an End with the right length) or fails.

### 2.6 Streams

One call may contain several concatenated *streams*, each ending with an End symbol and starting on a byte
boundary. `expected_len` is the total length of all of them; tables and window are not reset between streams
(matches may refer to bytes of an earlier stream, and symbol statistics carry over).

### 2.7 Errors in short

| Situation | Result |
|---|---|
| Input ends inside a code (class, extra bits, a match's fields) | `Truncated` |
| No input at all (even if `expected_len` is 0) | `Truncated` |
| Symbol index 274 or 275 (class 15 with extra bits 30 or 31) | `Corrupt` |
| A literal when the output is already `expected_len` long | `Corrupt` |
| A match that would exceed `expected_len` | `Corrupt` |
| End before `expected_len` is reached and no unfetched input byte remains | `Corrupt` |

### 2.8 Unspecified (not tested)

A match whose distance is 0 or reaches back farther than the bytes written so far; a symbol decoded more
than 32,767 times between two rebuilds (counter overflow); class widths of 32 or more; a rebuild that leaves
some symbol unreachable.

### 2.9 Vectors

`vectors/compress.tsv`: expected length, compressed stream (hex, or a file in `vectors/data/`), result
(`ok:<hex>`, `ok-file:<path>`, `err:truncated`, `err:corrupt`) and a note. The streams were produced by an
independent encoder, and cover: every literal, every match length (4..=521), every distance class boundary
with a full window, wrap-around of the window, rebuilds (including back to back, with ties, with different
class widths), several streams in one input, and the error cases above.

---

# Batch 2: file header, index files, string tables, archives

All multi-byte numbers in these formats are **little-endian**. `u32_at(d, o)` is the unsigned 32-bit number
stored in bytes `o..o+4` of `d`; `i32_at` is the same bytes read as a signed number. The error type of the
parsers is `FormatError`: `BadMagic(value)` (a magic number or version that is not accepted; the value found)
and `Truncated { offset }` (the data ends too early; `offset` as defined for each case below). Arithmetic on
sizes and offsets is **exact** (no wrap-around); a range is "beyond the data" when its end is greater than the
data length.

## 3. File header and index files

### 3.1 `parse_header(data)`

A data file starts with a 20-byte header.
1. If `data.len() < 20` the result is `Truncated { offset: 0 }`.
2. `magic = u32_at(data, 0)`. If `magic & 0xFFFFFFF0` is not `0x8000` the result is `BadMagic(magic)`
   (so magics `0x8000..=0x800F` are accepted; known values are 0x8000 data, 0x8001 palette, 0x8003 index).
3. The result is `FileHeader { magic, version: u32_at(data, 16) }`. Bytes 4..16 are ignored, and so is
   everything after byte 20.

### 3.2 `parse_index(data)`

An index file is a header followed by a table of 32-bit numbers. `parse_header(data)` is applied first (its
errors are returned unchanged). `offsets` is then the sequence `u32_at(data, 20 + 4k)` for every `k` for which
all four bytes exist; one to three bytes left over at the end are ignored. The result is
`Index { header, offsets }` (the offsets may be 0, meaning "no entry", and are not checked).

### 3.3 `entry_number(offset)`

The offsets in an index may point at another index file's table. `entry_number(offset)` is `None` if
`offset < 20`, otherwise `Some((offset - 20) / 4)` with integer division (rounding down).

## 4. String tables

A table file holds the text strings of one language: raw bytes, in the language's own 8-bit code page; this
code does not interpret the bytes.

### 4.1 `parse(data)` returns the list of strings

1. If `data.len() < 4` the result is `Truncated { offset: 0 }`.
2. `version = u32_at(data, 0)`. If it is not `0x15` the result is `BadMagic(version)`.
3. Starting with `p = 4`, repeat while `p + 4 <= data.len()`: let `n = i32_at(data, p)` and treat a negative
   `n` as 0; `p += 4`; if `p + n > data.len()` the result is `Truncated { offset: p }` (`p` after the 4 length
   bytes); otherwise the string is `data[p .. p + n]` (appended to the list) and `p += n`.
4. The list is returned. Fewer than 4 bytes left at the end are ignored. The number of strings is **not**
   checked: `parse` returns however many there are (a complete file has `COUNT = 0xEFC = 3836`).

### 4.2 `with_fallbacks(language, files)`

`files` is a list indexed by language number; each element is `None` (that language's file is not present) or
`Some(strings)` (a list as returned by `parse`). The result always has exactly `COUNT` (3836) strings. String
number `i` is determined like this: look at the files numbered `language`, `0` and `1`, in this order (the
language's own file first, even when `language` is 0 or 1; repeating a file is harmless). A file is considered
if the number exists in `files`, it is `Some`, and it has a string number `i`. The result is the first
considered string that is **not empty**; if there is none, the result is the empty string.

## 5. Archives

An archive stores many files in one file, with a directory at the end.

### 5.1 Layout

- The last 4 bytes of the archive are `dir`, the offset of the directory.
- The directory is at `dir`. It begins with a 24-byte header of six `u32` words: word 0 is ignored; word 1 is
  the version, which must be `0x1000`; word 2 `folder_blob_size`; word 3 `folder_count`; word 4
  `name_blob_size`; word 5 `file_count`.
- Then come the folder blob (`folder_blob_size` bytes), the name blob (`name_blob_size` bytes) and the file
  table: `file_count` rows of 24 bytes each. Row layout: `offset: u32` at 0, `stored_size: u32` at 4,
  `size: u32` at 8, `folder: i16` at 12 (two bytes, signed), two ignored bytes at 14, `flags: u32` at 16,
  `checksum: u32` at 20.
- File data lives anywhere before the directory, at the offsets given in the rows.

### 5.2 `Archive::parse(data)`

1. If `data.len() < 4`: `Truncated { offset: 0 }`. Let `dir = u32_at(data, data.len() - 4)`.
2. If `dir + 24 > data.len()`: `Truncated { offset: dir }`.
3. If the version word is not `0x1000`: `BadMagic(version)` (this is checked before anything after the header).
4. Let `p = dir + 24`. The folder blob is `data[p .. p + folder_blob_size]`; if it is beyond the data:
   `Truncated { offset: p }`. Then `p += folder_blob_size`. The name blob is `data[p .. p + name_blob_size]`; if
   beyond the data: `Truncated { offset: p }`. Then `p += name_blob_size`. The table is
   `data[p .. p + file_count * 24]`; if beyond the data: `Truncated { offset: p }`.
5. *Names.* A blob is cut at every 0 byte: a blob with `k` zero bytes yields `k + 1` pieces (the last one is
   whatever follows the last zero, possibly empty; an empty blob yields one empty piece). Only the first
   `folder_count` pieces of the folder blob and the first `file_count` pieces of the name blob are used (all of
   them if there are fewer). Each piece becomes a `String` by UTF-8 decoding in which every invalid sequence is
   replaced by U+FFFD (the standard "lossy" conversion).
6. *Entries.* Row `j` is paired with name piece `j`; the pairing stops at the shorter of the two (rows
   without a name piece are dropped). For each pair: `folder` is the row's signed 16-bit value; if
   `0 <= folder < (number of folder pieces used)` the entry's `path` is `folder_name + "\\" + name`
   (a single backslash), otherwise it is `name`. A folder name may be empty (the path then starts with a
   backslash). The other fields are copied from the row (`stored_size`, `size`, `flags`, `checksum`, `offset`).
   Entries are in table order.

### 5.3 `Archive::find(path)`

`want` is `path` with every `/` replaced by `\`. The result is the first entry (table order) whose `path`
equals `want` when ASCII letters are compared without regard to case (`A-Z` equals `a-z`; every other character
must match exactly). No match gives `None`. The returned reference points into the archive's own `entries`.

### 5.4 `Archive::read(entry)`

`entry` is one of the archive's entries.
1. If `entry.offset + entry.stored_size > data.len()` (the data being the whole archive): `Err(Range)`.
2. Otherwise, if `entry.flags & 2 != 0`: `Err(Scrambled)` (this format variant is not supported).
3. Otherwise `raw = data[offset .. offset + stored_size]`. If `flags & 1 != 0` the entry is compressed:
   the result is `decompress(raw, entry.size)` (section 2), with its error wrapped as `Decompress(error)`.
   Otherwise the result is a copy of `raw` (`size` is not used).

### 5.5 Unspecified (not tested)

An entry whose `offset + stored_size` does not fit in 32 bits; two entries with identical paths other than the
"first one wins" rule of `find`.

### 5.6 Vectors

`header.tsv` (headers, indices, entry numbers), `texts.tsv` and `texts-fallback.tsv`, `archive-parse.tsv`,
`archive-read.tsv`, `archive-find.tsv`. The archives and string tables are synthetic, built by the specification
side; the expected values are the reference implementation's results on them. Large inputs are files in
`vectors/data/`.


---

# Batch 3: palettes, PNG writer, sound

`FormatError` gains a third case for index files that point outside their data:
`BadOffset { index, offset }` (the entry number and the offending offset; see 8.2).

## 6. Palettes

A palette file holds 16-bit colour entries; the pixel format of the entries is not stored in the file but
follows from the file name's extension.

### 6.1 `format_from_extension(ext)`

Look at the **last character** of `ext`: `'4'` gives `Argb4444`, `'5'` gives `Rgb555`, `'6'` gives `Rgb565`;
any other last character, or an empty `ext`, gives `None`. (Only that one character matters: `"p26"`, `"pa6"`
and `"6"` are all `Rgb565`; `"P26"` is too, since its last character is `6`; `"pa56 "` ends in a space and
gives `None`.)

### 6.2 `to_rgba(format, color)` → `[red, green, blue, alpha]`

A component of `bits` bits with value `v` is scaled to 8 bits as
`scale(v, bits) = (v * 255 + (2^bits - 1) / 2) / (2^bits - 1)`, in integer arithmetic where `/` truncates
(so the rounding offset is 15 for 5 bits, 31 for 6 bits and 7 for 4 bits).

- `Rgb565`: red = `scale(c >> 11, 5)`, green = `scale((c >> 5) & 0x3F, 6)`, blue = `scale(c & 0x1F, 5)`,
  alpha = 255.
- `Rgb555`: red = `scale((c >> 10) & 0x1F, 5)`, green = `scale((c >> 5) & 0x1F, 5)`,
  blue = `scale(c & 0x1F, 5)`, alpha = 255 (bit 15 is ignored).
- `Argb4444`: red = `scale((c >> 8) & 0xF, 4)`, green = `scale((c >> 4) & 0xF, 4)`,
  blue = `scale(c & 0xF, 4)`, alpha = `scale(c >> 12, 4)`.

### 6.3 `parse_palette(data, format)`

`parse_header(data)` is applied first (its errors are returned). `colors` is then the list of the 16-bit
little-endian numbers at byte positions 20, 22, 24, … for as long as both bytes exist (an odd last byte is
ignored). The result is `PaletteFile { header, format, colors }`.

### 6.4 `PaletteFile::at(offset, len)`

Index files hold byte offsets into palette files. `at` returns `len` colours starting at the colour that the
offset designates:
1. If `offset < 20`: `Err(BadPalette { len: 0 })`.
2. `start = (offset - 20) / 2` (integer division, rounding down).
3. If `start + len` (computed without overflow) is more than the number of colours: `Err(BadPalette { len: remaining })` where `remaining`
   is the number of colours from `start` to the end (0 if `start` is at or beyond the end).
4. Otherwise `Ok(colors[start .. start + len])`.

### 6.5 Vectors

`vectors/data/palette-*.rgba`: the RGBA of every one of the 65,536 colours in each format (4 bytes per colour);
`palette.tsv`: extensions, palette files, and lookups.

## 7. PNG writer

`encode_rgba(width, height, rgba)` returns the bytes of a PNG file. Preconditions: `width >= 1`, `height >= 1`,
`rgba.len() == width * height * 4` (pixels row by row, top row first, each pixel red, green, blue, alpha);
the behaviour otherwise is unspecified. The output is **exactly** (byte for byte) as follows, so that it can be
compared with the vectors:

- The 8-byte signature `89 50 4E 47 0D 0A 1A 0A`, then the chunks `IHDR`, `IDAT`, `IEND`, in this order, and
  nothing else.
- Every chunk is: its data length (4 bytes, big-endian), its 4-letter type, its data, and the CRC-32 (4 bytes,
  big-endian) of type and data together. CRC-32 is the usual one of the PNG and zlib world (reflected
  polynomial `0xEDB88320`, initial value `0xFFFFFFFF`, result inverted).
- `IHDR` data (13 bytes): width (4, big-endian), height (4, big-endian), then the five bytes
  `08 06 00 00 00` (8 bits per channel, colour type 6 = RGBA, no special compression/filter/interlace).
- `IDAT` data is one zlib stream: the two bytes `78 01`; then the *raw data*, cut into consecutive blocks of
  at most 65,535 bytes (every block has 65,535 bytes except the last, which has the rest) and stored without
  compression, each block as: one byte that is `01` for the last block and `00` for the others, the block
  length (2 bytes, little-endian), the bitwise inverse of the block length (2 bytes, little-endian), the block's
  bytes; and finally the Adler-32 checksum of the raw data (4 bytes, big-endian; `a = 1, b = 0`, for each byte
  `a = (a + byte) mod 65521`, `b = (b + a) mod 65521`, value `b * 65536 + a`).
- The raw data is, for each row: a byte `00` (filter "none") followed by the row's `4 * width` bytes.
- `IEND` has no data.

Vectors: `vectors/png.tsv` with the expected files in `vectors/data/` (they were also checked with an
independent PNG decoder, and decode to the given pixels).

## 8. Sound

### 8.1 `decode_wav(wav)` → `Pcm { channels, rate, samples }`

`samples` are interleaved 16-bit signed samples (one sample per channel per frame). Errors: `NotWav`,
`Unsupported(tag)`, `Truncated`.

1. If `wav.len() < 12`, or bytes 0..4 are not `RIFF`, or bytes 8..12 are not `WAVE`: `NotWav`. (The size field
   at bytes 4..8 is ignored.)
2. *Chunks.* Set `p = 12`. While `p + 8 <= wav.len()`: the chunk id is bytes `p..p+4`, `len = u32_at(wav, p+4)`,
   and the chunk body is `wav[p+8 ..]` up to `len` bytes but **cut off at the end of the data** if `len` is more
   than is there. If the id is `fmt ` (with a trailing space) the body is remembered as the format chunk; if it
   is `data`, as the data chunk (a later chunk of the same id replaces an earlier one). Other chunks are
   skipped. Then `p = p + 8 + len + (len & 1)` (chunks are padded to even length; computed without overflow,
   so a huge `len` simply ends the loop).
3. If there is no format chunk or no data chunk: `Truncated`. If the format chunk is shorter than 16 bytes:
   `Truncated`.
4. From the format chunk: `tag = u16_at(fmt, 0)`, `channels = u16_at(fmt, 2)`, `rate = u32_at(fmt, 4)`,
   `align = u16_at(fmt, 12)` (the "block align"); the bytes 8..12 and 14..16 are not used.
5. **Tag 1** (16-bit PCM): `samples` are the data chunk's bytes read as 16-bit little-endian numbers, two bytes
   at a time (an odd last byte is ignored). `channels` and `rate` are returned as read, without checks.
6. **Tag `0x11`** (IMA ADPCM, 4 bits per sample):
   - If `channels == 0` or `align < 4 * channels`: `Truncated`.
   - The data chunk is cut into blocks of `align` bytes (the last block may be shorter). A block shorter than
     `4 * channels` bytes ends the decoding (it and everything after it are ignored).
   - For each block, with `ch = channels`: each channel `c` has a *predictor* (`i16_at(block, 4c)`) and a *step
     index* (`block[4c + 2]`, limited to at most 88; byte `4c + 3` is ignored).
   - The block produces, first, one frame containing every channel's predictor value. Then the block's
     remaining bytes `body = block[4*ch ..]` are processed in *groups* of `4 * ch` bytes (a final partial group
     is ignored): in group `g`, channel `c` owns the 4 bytes `body[(g*ch + c)*4 .. +4]`; these hold 8 nibbles,
     taken byte by byte, the **low nibble first**, then the high nibble. The `k`-th nibble (0..8) of channel
     `c` in group `g` gives the sample of channel `c` in frame `1 + g*8 + k` of this block. Frames of a block
     follow those of the previous block.
   - Decoding one nibble `n` for a channel: `step = STEP[index]`; `diff = ((2 * (n & 7) + 1) * step) >> 3`;
     if `n & 8` is set the predictor decreases by `diff`, otherwise it increases by `diff`; the predictor is then
     limited to `-32768..=32767`; the index becomes `index + INDEX[n]`, limited to `0..=88`; the sample is the
     predictor.
   - `INDEX` (16 entries): `-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8`.
   - `STEP` (89 entries): `7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50,
     55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408,
     449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499,
     2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635,
     13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767`.
   - The result has `channels` and `rate` as read.
7. **Any other tag**: `Unsupported(tag)`.

### 8.2 Sound banks: `SoundBank`

A bank is a sound file (`snd`) plus an index file (`sil`) of offsets into it.
- `SoundBank::parse(snd, sil)`: `parse_header(snd)` is applied to the sound file first (its errors are
  returned), then `parse_index(sil)` to the index (its errors are returned).
- `len()` is the number of index entries (as in 3.2); `is_empty()` is `len() == 0`.
- `wav(n)`: if `n >= len()` or entry `n` is 0, the result is `Ok(None)`. Otherwise let `o` be entry `n`: the
  8 bytes at `snd[o .. o + 8]` must exist, else `Err(BadOffset { index: n, offset: o })`; `length` is the second
  word of those 8 bytes (`u32_at(snd, o + 4)`; the first word is ignored); the sound is
  `snd[o + 8 .. o + 8 + length]`, and if that range is beyond the data: `Err(BadOffset { index: n, offset: o })`.
  Otherwise `Ok(Some(slice))`. (Offsets are used as they are; the file header's 20 bytes are not subtracted.)

### 8.3 Vectors

`wav.tsv` (WAV files, synthetic, with the decoded samples) and `sound-bank.tsv` (banks and every entry's
result). The ADPCM data is random bytes with random block headers (any nibbles are valid); the expected samples
are the reference implementation's results.

## 9. Dialog sets

`dialogs::parse(data)` reads a file that describes a set of dialogs (windows with controls). All numbers are
little-endian; `u16_at` / `u32_at` read 2 / 4 bytes at a position. Arithmetic is done with numbers wide enough
that nothing overflows (use 64-bit). The result is `Vec<Dialog>`; errors are `FormatError::Truncated { offset }`
and `FormatError::BadOffset { index, offset }`. There is **no magic number check** and no header type: this
file does not start with the 20-byte header of section 3.

**Layout.** The first 16 bytes are four 32-bit words: word 0 and words 2, 3 are ignored; word 1 is the number
of dialogs, `count`. After them come `count` 32-bit offsets (from the start of the file), one per dialog, i.e.
the offset of dialog `i` is `u32_at(data, 16 + 4 i)`.

**Dialog block** at offset `o`: 16 bytes of head, then `n` controls of 36 bytes each (`CONTROL_SIZE`).
- `surface = u16_at(o)`, `x = u16_at(o+2)`, `y = u16_at(o+4)`, `width = u16_at(o+6)`, `height = u16_at(o+8)`,
  `background = u16_at(o+10)`, `n = u16_at(o+12)`, `flags = u16_at(o+14)`.
- Control `j` is the 36 bytes `data[o+16+36j .. o+16+36j+36]` (`raw`), with `id = u16_at(raw, 0x0a)`,
  `kind = raw[0x18]`, `text_slot = raw[0x19]` read as a **signed** 8-bit number (`0xff` is `-1`),
  `flags = raw[0x1b]`. The other bytes of a control are not interpreted (but are returned in `raw`).

**Procedure.** If `data.len() < 16`: `Truncated { offset: 0 }`. Then for `i = 0, 1, ..., count-1`, in this order,
and stopping at the first error:
1. If `16 + 4i + 4 > data.len()`: `Truncated { offset: 16 + 4i }`.
2. `o = u32_at(data, 16 + 4i)`. If the 16 bytes `o .. o+16` are not all inside the file: `BadOffset { index: i,
   offset: o }`.
3. If the `n * 36` bytes `o+16 .. o+16+36n` are not all inside the file: `Truncated { offset: o + 16 }`.
4. Otherwise the dialog is produced.

With `count == 0` (and at least 16 bytes) the result is an empty list. Dialogs may share or overlap: nothing
checks that two offsets differ or that blocks are in order. Bytes after the last block, and gaps, are ignored.
Note that a large `count` costs nothing until an error is hit: the first missing offset ends the parse, so
the function must not reserve memory for `count` items beforehand.

### 9.1 Vectors

`gui-set.tsv`: synthetic files (random head values and random control bytes, built to the layout above), the
decoded dialogs and every control's 36 bytes, or the error.

## 10. Landscape texture files

### 10.1 `LandscapeFile::parse(data)`

A landscape texture file is the 20-byte header of section 3 (`parse_header`; its errors are returned
unchanged and the `header` field holds its result) followed by a chain of *records*. Each record is an 8-byte
head and then data:

| bytes | meaning |
|---|---|
| 0..2 | `flags` (16 bits) |
| 2 | ignored |
| 3 | `pages` (8 bits) |
| 4..8 | `size` (32 bits) |

Procedure: `p = 20`. While `p + 8 <= data.len()`:
1. `flags = u16_at(data, p)`, `pages = data[p+3]`, `size = u32_at(data, p+4)`, `start = p + 8`.
2. If `size == 0` the record's data is everything from `start` to the end of the file (possibly empty) and the
   record is the last one. Otherwise the data is `data[start .. start+size]`, and if that does not fit inside
   the file the result is `Truncated { offset: start }` (the whole parse fails; records found so far are
   dropped). Then `p = start + size`.

Fewer than 8 bytes left at the end are ignored. `Record::data` is a slice of the input (no copy is needed or
tested for, but the tests check *where* in the file it starts). Use 64-bit arithmetic; a `size` of 4 billion
simply does not fit.

### 10.2 `Record::is_software()` and `LandscapeFile::hardware_pages()`

`is_software()` is true when bit 1 (value `2`) of `flags` is set; other bits are irrelevant.

`hardware_pages()` returns `None` or `Some((side, pages))`:
1. Take the first record that is not software **and** whose `pages` is greater than 0. None: return `None`.
2. `page_bytes = data.len() / pages` (integer division). If `page_bytes == 0`: `None`.
3. `side` is the largest whole number with `side * side <= page_bytes / 2` (integer division by 2). If
   `side * side * 2 != page_bytes`: `None`. (So a page is a square of `side * side` 16-bit pixels with nothing
   left over; an odd `page_bytes` can never match.)
4. The pages are the consecutive `page_bytes`-byte pieces of the record data, **as many whole pieces as fit**,
   which is `data.len() / page_bytes` (at least `pages`, and more when the division left a remainder: for
   10 bytes and `pages = 4`, `page_bytes = 2`, `side = 1` and there are 5 pages). A last piece that is too short
   is ignored. Each page is its `side * side` 16-bit little-endian pixels, in file order.

The software records (flag bit 1) are never decoded: only their position and data are reported by `parse`.

### 10.3 Vectors

`landscape.tsv`: synthetic files; the parse result lists every record as `flags,pages,start,length` where
`start` is the position of the record's data in the file; the last column is the result of `hardware_pages()`.


## 11. Map files

A map file is a short head followed by a sequence of *chunks*. Each chunk has an encrypted 24-byte header and a
compressed payload. This section uses the cipher of section 1, the decompressor of section 2, and
`FormatError` (`BadMagic`, `Truncated`). All numbers are little-endian; use 64-bit arithmetic so nothing
overflows.

### 11.1 Layout

```
bytes 0..4   checksum word   (kept, never verified)
bytes 4..8   version         (must be 0x28 or 0x1f)
then chunks, back to back:
   24 bytes  chunk header, encrypted
   `stored` bytes of payload
```

The chunk header, after decryption, is six 32-bit words:

| word | bytes | field |
|---|---|---|
| 0 | 0..4 | `kind`: the type word |
| 1 | 4..8 | `stored`: number of payload bytes in the file |
| 2 | 8..12 | `size`: size of the payload after decompression |
| 3 | 12..16 | `checksum` (kept, never verified) |
| 4 | 16..20 | ignored |
| 5 | 20..24 | `param` (for several kinds the side length of the square map) |

Decryption: `apply_default` (section 1) on the 24 header bytes, with a **fresh** cipher for each header (the key
stream starts again from the beginning for every chunk; it is not continued from one header into the next).

### 11.2 `MapFile::parse(data)`

1. If `data.len() < 8`: `Truncated { offset: 0 }`.
2. `checksum = u32_at(data, 0)`, `version = u32_at(data, 4)`. If `version` is neither `0x28` nor `0x1f`:
   `BadMagic(version)`.
3. `p = 8`. While `p + 24 <= data.len()`:
   1. Decrypt the 24 bytes `data[p .. p+24]` into the header fields above.
   2. `start = p + 24`. The payload is `data[start .. start + stored]`. If it does not lie entirely inside the
      file: `Truncated { offset: start }` (the whole parse fails; chunks found so far are dropped).
   3. The chunk (header and a slice of the payload) is added to the list.
   4. If `kind & 0xFFFF` is `0` or `5`, stop: this is the end marker. (It **is** added to the list, with
      whatever payload it has; the high 16 bits of `kind` do not matter for this test.) Otherwise
      `p = start + stored` and continue.
4. If fewer than 24 bytes remain after the last chunk they are ignored, and so is everything after an end
   marker. A file that is just the 8-byte head gives zero chunks. The result is
   `MapFile { checksum, version, chunks }`.

### 11.3 Chunks

- `ChunkHeader::kind()` is `kind & 0xFFFF` as a `u16`; `index()` is `kind >> 16` as a `u16`.
- `Chunk::data()` is the decompression (section 2) of the payload `stored` slice with `expected_len = header.size`;
  its errors are returned unchanged.
- `MapFile::chunk(k)` is the first chunk in the list whose `header.kind()` equals `k` (the end marker can be
  found this way too: `chunk(0)`).
- `MapFile::size()` is `Some(param)` of the first chunk of kind `LANDSCAPE` (13), otherwise `None`.
- Kinds seen in real maps, for orientation only (nothing else is specified or tested): 1 general info, 2 player
  setup, 3 team setup (the index is the team number), 4 preview picture, 6 object grid, 13 landscape grid,
  7 settlers, 8 buildings, 9 goods piles, 15 description text. Chunks of other kinds are simply kept.

### 11.4 Records: `settlers()`, `buildings()`, `piles()`

Each takes the **first** chunk of kind 7 / 8 / 9 respectively, decompresses it, and cuts the data into
consecutive records of a fixed size (6, 20, 8 bytes); a partial record at the end is ignored. If there is no such
chunk, or its data does not decompress (any error), the result is an empty list. `x` and `y` are always
`u16_at(record, 0)` and `u16_at(record, 2)`.

| list | record size | fields |
|---|---|---|
| `settlers` | 6 | `kind = record[4]`, `player = record[5]` |
| `buildings` | 20 | `kind = record[4]`, `player = record[5]`, `occupied = record[6]`, `soldiers = record[7 .. 16]` (nine bytes); bytes 16..20 are ignored |
| `piles` | 8 | `good = record[4]`, `amount = record[5]`, `flag = record[6]`; byte 7 is ignored |

No value is checked or limited (types and positions are returned as they are).

### 11.5 Checksums

`crc16_swapped(data)` is a 16-bit cyclic redundancy check, bit by bit: `crc = 0`; for each byte `b`:
`crc ^= b`, then 8 times: if `crc & 1` is set, `crc = (crc >> 1) ^ 0xA001`, else `crc = crc >> 1`. The result is
`crc` with its two bytes swapped (`crc.swap_bytes()`), as a `u32`. (For the 9 ASCII bytes `123456789` the crc
is `0xBB3D`, so the result is `0x3DBB`.)

`checksum(file)` is `crc16_swapped` of the bytes of `file` after the first 8 (all of `file` from index 8 on);
for a file shorter than 8 bytes the data is empty and the result is `0`. This is the value a map file's
checksum word would hold; the parser does not compare them.

### 11.6 Vectors

`map.tsv`: synthetic map files (built by the specification side, with compressed chunks, damaged chunks and
every framing error), each with the parse result, `size()`, the three record lists, every chunk's decompressed
data (or its error) and `checksum(file)`. `map-checksum.tsv`: `crc16_swapped` and `checksum` over synthetic
byte strings.


## 12. Sprite files

A sprite file holds images (small bitmaps with a position offset). Each pixel is either transparent, a shadow,
or an index into a palette. Section 3 applies to the file header (`parse_header`, `entry_number`); the images
follow it from byte 20 on. All numbers are little-endian, `u16_at(d, p)` reads two bytes, and `i8` / `i16` are
signed. Positions are `usize`; compute `p + n` without overflow (use 64-bit arithmetic).

### 12.1 `ImageHeader::parse(data, at)`

An image starts with a header in one of two forms. Which form is used depends on the first 16-bit number of the
header: `w0 = u16_at(data, at)`.

1. If `at + 2 > data.len()`: `Truncated { offset: at }`.
2. If `w0 >= 861` (**short form**, 8 bytes): if `at + 8 > data.len()`: `Truncated { offset: at }`. Otherwise
   `width = data[at]`, `height = data[at+1]` (each 0..255), `x = data[at+2]` as signed 8-bit, `y = data[at+3]`
   as signed 8-bit, `flags = u16_at(data, at+4)`, `extra = u16_at(data, at+6)`, `size = 8`.
3. Otherwise (`w0 <= 860`, **long form**, 12 bytes): if `at + 12 > data.len()`: `Truncated { offset: at }`.
   Otherwise `width = u16_at(data, at)`, `height = u16_at(data, at+2)`, `x = u16_at(data, at+4)` as signed 16-bit,
   `y = u16_at(data, at+6)` as signed 16-bit, `flags = u16_at(data, at+8)`, `extra = u16_at(data, at+10)`,
   `size = 12`.

(In the short form the first number `w0` is `width + 256 * height`, so the form is "short" when that is at least
861, e.g. `height >= 4`, or `height == 3` and `width >= 94`; the test is on `w0` itself, whatever the fields turn
out to be.)

### 12.2 `ImageHeader::is_raw()`

True when `size == 12` and bit `0x20` (`FLAG_RAW`) of `flags` is set. (A short-form header is never raw, even
with that flag bit set; other flag bits are irrelevant.)

### 12.3 `Image::at(data, offset)` and `end()`

`header = ImageHeader::parse(data, offset)?` (its errors are returned), `start = offset + header.size`.

- **Raw image** (`header.is_raw()`): the image data is `data[start .. start + width * height]`. If that does not
  lie entirely inside the file: `Truncated { offset: start }`.
- **Run-length image**: scan from `p = start`: read a byte `b` (if `p >= data.len()`: `Truncated { offset: p }`),
  `p += 1`. If `b >= 2` it is a pixel: continue the scan. Otherwise (`b` is 0 or 1) read a second byte `n` (if
  `p >= data.len()`: `Truncated { offset: p }`), `p += 1`; if `n == 0` the scan ends, otherwise (a run) it
  continues. The image data is `data[start .. p]`, i.e. including the end marker `00 00` or `01 00`. The width
  and height play no part in finding the end.

The result is `Image { header, offset, data }`. `end()` is `offset + header.size + data.len()`, the position just
after the image.

### 12.4 `Image::pixels()`

The result has exactly `total = width * height` pixels, row by row (left to right, top to bottom; runs may
continue onto the next row). Images with more than 70,000 pixels are never decoded by the tests.

- **Raw image**: one pixel per data byte: `0` is `Transparent`, any other value `b` is `Index(b)`.
- **Run-length image**: start with no pixels and read the data bytes in order, **while fewer than `total` pixels
  have been produced**:
  - the data is used up: stop;
  - a byte `b >= 2` gives one `Index(b)`;
  - a byte `b` of 0 or 1 is followed by a count byte `n` (a missing count counts as 0). If `n == 0` stop
    (end marker). Otherwise it gives `n` pixels, `Transparent` for `b == 0` and `Shadow` for `b == 1`, but never
    more than the pixels still missing to `total` (a longer run is cut off).
  - Finally, if fewer than `total` pixels were produced, the rest are `Transparent`. Data after the pixel that
    completes the image (including the end marker) is not looked at.

### 12.5 `GfxFile`

- `GfxFile::parse(data)`: `parse_header(data)` (section 3.1; its errors are returned unchanged), keeping the
  header and the data.
- `image_at(offset)`: `Image::at(data, offset as usize)`.
- `images()`: an iterator over the images stored back to back. It starts at position 20. Each step: if the
  position is at or beyond the end of the data, the iterator is finished. Otherwise it reads `Image::at` at the
  position and yields the result; after an `Ok(image)` the position becomes `image.end()`, after an `Err` the
  position becomes the end of the data (so an error is the last item).
- `indexed(index)`: for each offset `o` of `index.offsets` in order (entry number `i`):
  - `o == 0`: the entry is `None`;
  - `o >= data.len()`: the whole call fails with `BadOffset { index: i, offset: o }`;
  - otherwise `image_at(o)`; its error ends the call with that error, its image is `Some(image)`.

  The first failing entry decides; the result has one element per offset. (An offset inside the 20-byte file
  header is not an error as such: an image is read from there like from anywhere else.)

### 12.6 Vectors

`gfx.tsv`: synthetic image headers and images (both forms, raw and run-length, all kinds of damage), whole
files, indexed access and animation tables (section 13); the file explains the format of each row kind in its
first lines. Pixel lists are spelled `T` (transparent), `S` (shadow) and `[hh]` (palette index in hex).

## 13. Animation tables

Three index files cooperate: a *job* index lists, for each job, where its directions start in the *direction*
index; a direction entry says where its frames start in the *image* index; a direction's frames are the image
index entries from there up to where the next direction (of any job) starts. All offsets are byte offsets
into the table they point at, and `entry(o)` (= `entry_number(o)` of section 3.3) turns one into an entry number:
`None` if `o < 20`, else `(o - 20) / 4` (rounded down).

`job_frames(jobs, directions, image_count)` takes two `Index` values (only their `offsets` are used) and the
number of entries of the image index. It returns, for each job (in the order of `jobs.offsets`), a list of
directions, each a list of image entry numbers.

Definitions (all lists of whole numbers; "the smallest element greater than `v`" is the least element that is
strictly greater than `v`):

1. `starts`: the numbers `entry(o)` for every `o` in `directions.offsets` for which it is `Some`, plus
   `image_count`; sorted ascending, duplicates removed.
2. `frames_from(first)`: let `next` be the smallest element of `starts` greater than `first`, or `image_count`
   if there is none. The result is the list `first, first+1, ..., next-1` (empty when `next <= first`).
3. `job_starts`: the numbers `entry(o)` for every `o` in `jobs.offsets` for which it is `Some` (in any order,
   duplicates allowed), plus the number of entries of `directions.offsets`.
4. For a job with offset `o`: if `entry(o)` is `None`, the job has no directions (an empty list). Otherwise let
   `first = entry(o)`; let `next` be the smallest element of `job_starts` greater than `first` (there is always
   one when `first` is less than the number of entries of `directions.offsets`; otherwise treat `next` as that
   number). The job's directions are the entries `d = first, first+1, ..., min(next, direction count) - 1` of
   `directions.offsets` (none if that range is empty), and for each such `d` with offset `q = directions.offsets[d]`:
   - `q == 0`, or `entry(q)` is `None`: an empty list of frames;
   - otherwise `frames_from(entry(q))`.

Results that would need more than a million numbers in total are not tested (the offsets in the tables are
arbitrary 32-bit numbers, so a nonsensical table could ask for billions). Everything else (tables pointing
beyond the end, repeated offsets, unsorted tables) follows from the rules above.

### 13.1 Vectors

Rows of kind `jobs` in `gfx.tsv`: the three tables are given as comma-separated offset lists and the result is
`J(<direction>;<direction>...)` per job, separated by spaces, each direction a comma-separated list of numbers.


## 14. Terrain tables

The landscape of a map is a grid of tiles of four bytes each (byte 0: height, byte 1: terrain type, byte 2:
light and flags, byte 3: fog and flags). This section gives the constant tables that classify terrain types and
the colour a tile has on the small overview map.

### 14.1 Tables

`vectors/terrain.tsv` holds the tables as comma-separated numbers, one table per row (the first lines of the file
explain the layout). **Copy them into the code as constants** with these exact names and types:

| name | type | content |
|---|---|---|
| `TERRAIN_COLORS` | `[[u8; 3]; 256]` | overview colour (red, green, blue) of each terrain type |
| `TEXTURE_CLASS` | `[u8; 256]` | texture class of each terrain type |
| `CLASS_PAGE` | `[i8; 256]` | texture page of each class (-1: none) |
| `TRANSITION_INDEX` | `[u8; 256]` | transition index of each class |
| `TEXELS_PER_TILE` | `u32` | `32` |
| `TRANSITIONS` | `&[(u8, u8, [u8; 4])]` | the list of transitions, in file order: `(pair index, odd index, [four values])` |

The row `colors` lists the 768 numbers of `TERRAIN_COLORS` entry by entry (red, green, blue of type 0, then of
type 1, ...). The row `transitions` lists six numbers per transition (`pair`, `odd`, then the four values).

### 14.2 `transition(pair, odd)`

`pair` and `odd` are texture classes (0..255). Let `a = TRANSITION_INDEX[pair]` and `b = TRANSITION_INDEX[odd]`.
The result is the four values of the **first** entry of `TRANSITIONS` whose first number equals `a` and whose
second equals `b`, or `None` if there is none. (Note that this compares the *indices*, so many different classes
map to index 0.)

### 14.3 `minimap_color(tile)`

`tile` is `[u8; 4]`. Byte 0 is not used. Compute with signed 32-bit integers:

1. `c = TERRAIN_COLORS[tile[1]]` (three numbers), and `shade = (tile[2] & 0x0F) * 8 - 64`. Add `shade` to each
   of the three components.
2. If `tile[2] & 0x40` is set: `grey = (c0 + c1 + 2 * c2) / 4` using **integer division that truncates toward
   zero** (the sum can be negative), and all three components become `grey`.
3. `fog = (tile[3] >> 3) & 7`. If `fog == 7`: the result is `[9, 9, 9]` (nothing else is computed). If `fog == 0`:
   nothing changes. Otherwise each component is divided by 2 (again truncating toward zero).
4. Each component is limited to the range `9..=255` and returned as `u8`.

The other bits of `tile[2]` and `tile[3]` (and byte 0) do not matter.

### 14.4 Vectors

`terrain.tsv` (the tables); `data/terrain-transition.bin` (5 bytes per pair of classes `(pair, odd)`, in the
order `pair * 256 + odd`: a byte 1 if there is an entry or 0, then the four values or four zeros);
`data/terrain-minimap.bin` (3 bytes per tile, for every terrain type 0..255, every value of byte 2, and the eight
fog values; the order is `((type * 256 + byte2) * 8 + fog) * 3`; the tiles are built with byte 0 and the low
bits of byte 3 filled with unrelated values, see `tests/terrain.rs`).

## 15. The game folder

`GameFiles` finds the files of the game: first loose files in a folder, then files inside up to two archives
(section 5) stored in that folder.

### 15.1 `GameFiles::open(root)`

`root` is a folder path (it may not exist; `open` still succeeds: `open` returns `Ok` in every case the
specification covers). It looks at the two file names `game.lib` and `gfx.lib`, in this order, **exactly these
spellings** inside `root`. Each one that can be read as a whole file **and** is accepted by `Archive::parse`
(section 5.2) is kept (its bytes are held in memory); the others are skipped silently. The kept archives stay in
that order.

### 15.2 `GameFiles::read(path)`

`path` is text such as `Gfx\10.gil` or `Gfx/10.gil`.
1. Replace every `\` by `/` and try to read the file with that relative name inside `root`; if the read
   succeeds (a regular readable file), return its contents. (A folder of that name does not count: the read
   fails.) The case of the name is whatever the operating system's file system makes of it.
2. Otherwise, for each kept archive in order: parse it and look the **original** `path` up with `find`
   (section 5.3, which accepts both slashes and ignores letter case); if an entry is found and `Archive::read`
   (section 5.4) gives its contents, return them; otherwise go on to the next archive.
3. Otherwise `None`.

Not specified (not tested): paths that are absolute or contain `..`; whether a failing `read` in the first
archive moves on to the second (the description above says it does).

### 15.3 Vectors

`tests/game_files.rs` builds folders in the temporary directory from the archive vector files of section 5
and checks the rules above.
