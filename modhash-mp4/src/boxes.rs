//! ISO-BMFF box walking (ISO/IEC 14496-12 §4.2).
//!
//! A box is `size:u32, type:u32, payload`. `size == 1` means the real size is
//! the `largesize:u64` that follows the type; `size == 0` means the box runs
//! to the end of the enclosing range (legal at top level and for `mdat`).
//! `uuid` boxes carry a 16-byte extended type we skip past without
//! interpreting, because the demuxer never reads one.

use modhash_primitives::{Error, Result};

/// Four bytes naming a box or a brand, e.g. `b"moov"`.
pub type Four = [u8; 4];

/// One box header plus the byte range of its payload inside the file.
///
/// `payload` indexes the same slice the reader was created over, so walking
/// never copies. `largesize` records whether the box used the 64-bit form, for
/// callers that care about the encoding itself (tests do).
#[derive(Clone, Debug)]
pub struct BoxHead {
    /// The box type, e.g. `b"trak"`.
    pub four: Four,
    /// Byte range of the payload, absolute in the file.
    pub payload: core::ops::Range<usize>,
    /// Byte offset where this box starts (size field).
    pub offset: usize,
    /// Total size of the box including its header.
    pub size: u64,
    /// True when the box used the `size == 1` largesize encoding.
    pub largesize: bool,
}

/// A bounds-checked byte cursor over one buffer.
///
/// Every read advances `pos` and every multi-byte field is big-endian, the
/// only byte order ISO-BMFF uses. Nothing here allocates; the cursor is a
/// position plus the original slice.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Starts a cursor at the beginning of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Number of bytes still readable.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Absolute position of the cursor in the buffer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The whole underlying buffer.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Reads `n` bytes or fails [`Error::Truncated`] naming `what`.
    pub fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::truncated(what, self.pos + n, self.data.len()));
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Reads one unsigned byte.
    pub fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    /// Reads a big-endian `u16`.
    pub fn u16(&mut self, what: &'static str) -> Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Reads a big-endian `u24` into a `u32`.
    pub fn u24(&mut self, what: &'static str) -> Result<u32> {
        let b = self.take(3, what)?;
        Ok((u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]))
    }

    /// Reads a big-endian `u32`.
    pub fn u32(&mut self, what: &'static str) -> Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads a big-endian `i32`.
    pub fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }

    /// Reads a big-endian `u64`.
    pub fn u64(&mut self, what: &'static str) -> Result<u64> {
        let b = self.take(8, what)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Reads a four-byte tag.
    pub fn four(&mut self, what: &'static str) -> Result<Four> {
        let b = self.take(4, what)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// Skips `n` bytes, failing if they are not there.
    pub fn skip(&mut self, n: usize, what: &'static str) -> Result<()> {
        self.take(n, what).map(|_| ())
    }
}

/// Reads one box header at `offset` inside `data`, `end` bounding the
/// enclosing range.
///
/// Returns the parsed header. Errors:
///
/// * [`Error::Truncated`] when fewer than 8 header bytes remain or the
///   declared size overruns `end`.
/// * [`Error::BadValue`] when the declared size is smaller than the header
///   itself (a file that would loop a naive walker).
pub fn read_box(data: &[u8], offset: usize, end: usize) -> Result<BoxHead> {
    debug_assert!(offset <= end && end <= data.len());
    let avail = end - offset;
    if avail < 8 {
        return Err(Error::truncated("box header", offset + 8, end));
    }
    let size32 = u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]);
    let four: Four = [
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    let mut head_len = if four == *b"uuid" { 24usize } else { 8usize };
    let mut largesize = false;
    let size: u64 = match size32 {
        // "To end of the enclosing range": the box is whatever is left.
        0 => avail as u64,
        1 => {
            largesize = true;
            if avail < head_len + 8 {
                return Err(Error::truncated(
                    "box largesize",
                    offset + head_len + 8,
                    end,
                ));
            }
            let hi = offset + head_len;
            head_len += 8;
            u64::from_be_bytes([
                data[hi],
                data[hi + 1],
                data[hi + 2],
                data[hi + 3],
                data[hi + 4],
                data[hi + 5],
                data[hi + 6],
                data[hi + 7],
            ])
        }
        n => u64::from(n),
    };
    if size < head_len as u64 {
        return Err(Error::BadValue("box size smaller than its header"));
    }
    if size > avail as u64 {
        return Err(Error::truncated(
            "box",
            offset.saturating_add(size as usize),
            end,
        ));
    }
    let size_usize = size as usize;
    Ok(BoxHead {
        four,
        payload: offset + head_len..offset + size_usize,
        offset,
        size,
        largesize,
    })
}

/// Iterates the immediate child boxes of `range` inside `data`.
///
/// Container boxes are walked by handing the box's `payload` range back to a
/// fresh `Boxes`. The iterator stops with one [`Error::Truncated`] item as
/// soon as a child header or size is malformed; a trailing sliver shorter
/// than a header also errors, because a container that claims children must
/// contain whole boxes.
pub struct Boxes<'a> {
    data: &'a [u8],
    next: usize,
    end: usize,
}

impl<'a> Boxes<'a> {
    /// Iterates boxes occupying `data[range]` exactly.
    pub fn new(data: &'a [u8], range: core::ops::Range<usize>) -> Self {
        Self {
            data,
            next: range.start,
            end: range.end,
        }
    }

    /// Finds the first child box of type `four` in `range`.
    pub fn find(
        data: &'a [u8],
        range: core::ops::Range<usize>,
        four: Four,
    ) -> Result<Option<BoxHead>> {
        for head in Self::new(data, range) {
            let head = head?;
            if head.four == four {
                return Ok(Some(head));
            }
        }
        Ok(None)
    }
}

impl Iterator for Boxes<'_> {
    type Item = Result<BoxHead>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        match read_box(self.data, self.next, self.end) {
            Ok(head) => {
                self.next += head.size as usize;
                Some(Ok(head))
            }
            Err(e) => {
                self.next = self.end;
                Some(Err(e))
            }
        }
    }
}

/// Reads the `(version:u8, flags:u24)` word that opens every full box.
pub fn full_box(reader: &mut Reader<'_>, what: &'static str) -> Result<(u8, u32)> {
    let version = reader.u8(what)?;
    let flags = reader.u24(what)?;
    Ok((version, flags))
}
