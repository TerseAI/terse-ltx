use std::io::{self, Read, Write};
use terse_ltx::{Decoder, Encoder, compact};

const BLOCK: &[u8] = include_bytes!("fixtures/0-1.ltx");
const FRAME: &[u8] = include_bytes!("fixtures/0-frame.ltx");

#[test]
fn supports_short_and_interrupted_reads_and_writes() -> io::Result<()> {
    for input in [BLOCK, FRAME] {
        for chunk in [1, 7, 64] {
            let mut output = Fragmented::new(Vec::new(), chunk, None, true);
            compact(vec![Fragmented::new(input, chunk, None, true)], &mut output)?;
            let mut actual = Decoder::new(output.inner.as_slice())?;
            let mut expected = Decoder::new(BLOCK)?;
            assert_eq!(actual.header(), expected.header());
            while let Some(page) = expected.next_page()? {
                assert_eq!(actual.next_page()?, Some(page));
            }
            assert!(actual.next_page()?.is_none());
            assert_eq!(
                actual.trailer().unwrap().post_apply_checksum,
                expected.trailer().unwrap().post_apply_checksum
            );
        }
    }
    Ok(())
}

#[test]
fn propagates_reader_and_writer_failures_at_every_boundary() -> io::Result<()> {
    for input in [BLOCK, FRAME] {
        for offset in 0..=input.len() {
            let error = compact(
                vec![Fragmented::new(input, 17, Some(offset), false)],
                Vec::new(),
            )
            .unwrap_err();
            assert_eq!(
                error.kind(),
                io::ErrorKind::Other,
                "read offset {offset}: {error}"
            );
            assert_eq!(error.to_string(), "injected I/O failure");
        }
    }
    let mut expected = Vec::new();
    compact(vec![BLOCK], &mut expected)?;
    for offset in 0..expected.len() {
        let error = compact(
            vec![BLOCK],
            Fragmented::new(Vec::new(), 17, Some(offset), false),
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::Other,
            "write offset {offset}: {error}"
        );
    }
    Ok(())
}

#[test]
fn rejects_zero_progress_writers() -> io::Result<()> {
    let header = *Decoder::new(BLOCK)?.header();
    let error = Encoder::new(Fragmented::new(Vec::new(), 0, None, false), header)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    Ok(())
}

struct Fragmented<T> {
    inner: T,
    chunk: usize,
    position: usize,
    fail_at: Option<usize>,
    interrupts: bool,
    interrupt_next: bool,
}

impl<T> Fragmented<T> {
    fn new(inner: T, chunk: usize, fail_at: Option<usize>, interrupts: bool) -> Self {
        Self {
            inner,
            chunk,
            position: 0,
            fail_at,
            interrupts,
            interrupt_next: interrupts,
        }
    }

    fn length(&mut self, requested: usize) -> io::Result<usize> {
        if self.interrupt_next {
            self.interrupt_next = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.fail_at == Some(self.position) {
            return Err(io::Error::other("injected I/O failure"));
        }
        self.interrupt_next = self.interrupts;
        Ok(requested
            .min(self.chunk)
            .min(self.fail_at.map_or(usize::MAX, |end| end - self.position)))
    }
}

impl<T: Read> Read for Fragmented<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let length = self.length(bytes.len())?;
        let count = self.inner.read(&mut bytes[..length])?;
        self.position += count;
        Ok(count)
    }
}

impl<T: Write> Write for Fragmented<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self.length(bytes.len())?;
        let count = self.inner.write(&bytes[..length])?;
        self.position += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
