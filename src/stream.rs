//! Input as an ordered stream of chunks.
//!
//! The input is cut into chunks of whole records (the cut is always at a record separator).
//! Every chunk has a sequence number and knows how many records precede it, so a chunk can be
//! processed on its own and its results put back in order afterwards. The sequential run uses
//! the same chunks one after another; the parallel run (not written yet) hands them to
//! workers and merges the results with [`OrderedMerger`].

use std::collections::BTreeMap;
use std::io::{self, Read};

/// A run of whole records.
#[derive(Debug, PartialEq, Eq)]
pub struct Chunk {
    /// 0, 1, 2, ... in input order.
    pub seq: u64,
    /// Records before this chunk in the same input.
    pub first_record: u64,
    /// Number of records in the chunk (the last one may lack its separator).
    pub records: u64,
    /// The records, each followed by its separator except possibly the last one of the input.
    pub data: Vec<u8>,
    /// Whether the input ended with the last record of this chunk.
    pub last: bool,
}

/// Cuts a byte stream into [`Chunk`]s at record separators.
pub struct ChunkReader<R: Read> {
    reader: R,
    separator: u8,
    block: usize,
    carry: Vec<u8>,
    seq: u64,
    records: u64,
    eof: bool,
}

impl<R: Read> ChunkReader<R> {
    /// `block` is the size of one read; a chunk is at least one read and at most a read plus
    /// the unfinished record it completes. A record longer than a block is never split.
    pub fn new(reader: R, separator: u8, block: usize) -> Self {
        ChunkReader {
            reader,
            separator,
            block: block.max(1),
            carry: Vec::new(),
            seq: 0,
            records: 0,
            eof: false,
        }
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    /// The next chunk, or `None` at the end of the input.
    pub fn next_chunk(&mut self) -> io::Result<Option<Chunk>> {
        loop {
            if self.eof {
                if self.carry.is_empty() {
                    return Ok(None);
                }
                // The tail has no separator: it is the unterminated last record.
                let data = std::mem::take(&mut self.carry);
                return Ok(Some(self.emit(data, true)));
            }
            let old = self.carry.len();
            self.carry.resize(old + self.block, 0);
            let n = match self.reader.read(&mut self.carry[old..]) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    self.carry.truncate(old);
                    continue;
                }
                Err(e) => {
                    self.carry.truncate(old);
                    return Err(e);
                }
            };
            self.carry.truncate(old + n);
            if n == 0 {
                self.eof = true;
                continue;
            }
            // Cut after the last separator, keep the unfinished record for the next read.
            if let Some(at) = crate::search::memrchr(self.separator, &self.carry[old..]) {
                let cut = old + at + 1;
                let rest = self.carry.split_off(cut);
                let data = std::mem::replace(&mut self.carry, rest);
                return Ok(Some(self.emit(data, false)));
            }
        }
    }

    fn emit(&mut self, data: Vec<u8>, unterminated_tail: bool) -> Chunk {
        let mut records = crate::search::count(self.separator, &data) as u64;
        if unterminated_tail {
            records += 1;
        }
        let chunk = Chunk {
            seq: self.seq,
            first_record: self.records,
            records,
            data,
            last: unterminated_tail,
        };
        self.seq += 1;
        self.records += records;
        chunk
    }
}

/// Puts items that arrive out of order back in sequence.
///
/// `push` accepts an item with its sequence number; `next_ready` returns the item whose turn
/// it is, if it has arrived. `pending` is the number of items held back, which a caller can
/// use to bound the work in flight.
pub struct OrderedMerger<T> {
    next: u64,
    waiting: BTreeMap<u64, T>,
}

impl<T> Default for OrderedMerger<T> {
    fn default() -> Self {
        OrderedMerger {
            next: 0,
            waiting: BTreeMap::new(),
        }
    }
}

impl<T> OrderedMerger<T> {
    pub fn push(&mut self, seq: u64, item: T) {
        self.waiting.insert(seq, item);
    }

    pub fn next_ready(&mut self) -> Option<T> {
        let item = self.waiting.remove(&self.next)?;
        self.next += 1;
        Some(item)
    }

    pub fn pending(&self) -> usize {
        self.waiting.len()
    }

    /// The sequence number whose item is needed next.
    pub fn next_seq(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that returns its data in pieces of the given sizes, to exercise every cut.
    struct Pieces<'a> {
        data: &'a [u8],
        sizes: Vec<usize>,
        at: usize,
    }

    impl Read for Pieces<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let size = self.sizes[self.at % self.sizes.len()];
            self.at += 1;
            let n = size.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn chunks(data: &[u8], block: usize, sizes: Vec<usize>) -> Vec<Chunk> {
        let mut reader = ChunkReader::new(Pieces { data, sizes, at: 0 }, b'\n', block);
        let mut out = Vec::new();
        while let Some(c) = reader.next_chunk().unwrap() {
            out.push(c);
        }
        out
    }

    #[test]
    fn chunks_reassemble_to_the_input_and_cut_at_separators() {
        let data = b"one\ntwo\n\nthree\nfour";
        for block in [1, 2, 3, 5, 8, 64] {
            for sizes in [vec![1], vec![2, 3], vec![7], vec![100]] {
                let got = chunks(data, block, sizes.clone());
                let joined: Vec<u8> = got.iter().flat_map(|c| c.data.clone()).collect();
                assert_eq!(joined, data, "block {block} sizes {sizes:?}");
                for (i, c) in got.iter().enumerate() {
                    assert_eq!(c.seq, i as u64);
                    if i + 1 < got.len() {
                        assert_eq!(c.data.last(), Some(&b'\n'), "cut inside a record");
                    }
                }
                assert_eq!(got.iter().map(|c| c.records).sum::<u64>(), 5);
                let mut expected_first = 0;
                for c in &got {
                    assert_eq!(c.first_record, expected_first);
                    expected_first += c.records;
                }
                assert!(
                    got.last().unwrap().last,
                    "the unterminated tail ends the input"
                );
            }
        }
    }

    /// A reader that is interrupted once, then fails, around its data.
    struct Flaky<'a> {
        data: &'a [u8],
        calls: usize,
    }

    impl Read for Flaky<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            match self.calls {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    let n = self.data.len().min(buf.len());
                    buf[..n].copy_from_slice(&self.data[..n]);
                    self.data = &self.data[n..];
                    Ok(n)
                }
                _ => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            }
        }
    }

    #[test]
    fn an_interrupted_read_is_retried_and_other_errors_are_reported() {
        let mut reader = ChunkReader::new(
            Flaky {
                data: b"a\nb\n",
                calls: 0,
            },
            b'\n',
            16,
        );
        let first = reader.next_chunk().unwrap().unwrap();
        assert_eq!(first.data, b"a\nb\n");
        assert_eq!(
            reader.next_chunk().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_terminated_input_has_no_unterminated_tail() {
        let got = chunks(b"a\nb\n", 3, vec![2]);
        assert!(got.iter().all(|c| !c.last));
        assert_eq!(got.iter().map(|c| c.records).sum::<u64>(), 2);
    }

    #[test]
    fn empty_input_has_no_chunks() {
        assert!(chunks(b"", 4, vec![4]).is_empty());
    }

    #[test]
    fn a_record_longer_than_a_block_is_not_split() {
        let data = b"abcdefghij\nk\n";
        let got = chunks(data, 3, vec![3]);
        assert_eq!(got.iter().map(|c| c.records).sum::<u64>(), 2);
        assert!(got.iter().any(|c| c.data.starts_with(b"abcdefghij\n")));
    }

    #[test]
    fn the_merger_restores_order() {
        let mut m = OrderedMerger::default();
        m.push(2, "c");
        m.push(0, "a");
        assert_eq!(m.next_ready(), Some("a"));
        assert_eq!(m.next_ready(), None);
        assert_eq!(m.pending(), 1);
        m.push(1, "b");
        assert_eq!(m.next_ready(), Some("b"));
        assert_eq!(m.next_ready(), Some("c"));
        assert_eq!(m.next_ready(), None);
        assert_eq!(m.next_seq(), 3);
    }
}
