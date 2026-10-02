use std::io::{self, Write};
use terse_ltx::{CHECKSUM_FLAG, Decoder, Encoder, Header, NO_CHECKSUM, compact};

#[test]
fn validates_header_contract() {
    let valid = delta_header();
    for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        Header {
            page_size: size,
            ..valid
        }
        .validate()
        .unwrap();
    }
    for invalid in [
        Header { flags: 1, ..valid },
        Header {
            flags: u32::MAX,
            ..valid
        },
        Header {
            page_size: 0,
            ..valid
        },
        Header {
            page_size: 256,
            ..valid
        },
        Header {
            page_size: 513,
            ..valid
        },
        Header {
            page_size: 131072,
            ..valid
        },
        Header {
            min_txid: 0,
            ..valid
        },
        Header {
            max_txid: 0,
            ..valid
        },
        Header {
            min_txid: 3,
            max_txid: 2,
            ..valid
        },
        Header {
            wal_offset: -1,
            ..valid
        },
        Header {
            wal_size: -1,
            ..valid
        },
        Header {
            wal_size: 1,
            ..valid
        },
        Header {
            wal_salt: [1, 0],
            ..valid
        },
        Header {
            wal_salt: [0, 1],
            ..valid
        },
        Header {
            flags: 0,
            pre_apply_checksum: 0,
            ..valid
        },
        Header {
            flags: 0,
            pre_apply_checksum: 1,
            ..valid
        },
        Header {
            flags: NO_CHECKSUM,
            pre_apply_checksum: CHECKSUM_FLAG,
            ..valid
        },
        Header {
            flags: 0,
            min_txid: 1,
            pre_apply_checksum: CHECKSUM_FLAG,
            ..valid
        },
    ] {
        assert!(invalid.validate().is_err(), "accepted {invalid:?}");
        assert!(Encoder::new(Vec::new(), invalid).is_err());
    }
    for valid in [
        Header {
            flags: 0,
            min_txid: 1,
            ..valid
        },
        Header {
            flags: 0,
            pre_apply_checksum: CHECKSUM_FLAG,
            ..valid
        },
        Header {
            wal_offset: 32,
            wal_size: 536,
            wal_salt: [1, 2],
            ..valid
        },
        Header {
            min_txid: u64::MAX,
            max_txid: u64::MAX,
            timestamp: i64::MIN,
            ..valid
        },
    ] {
        valid.validate().unwrap();
    }
}

#[test]
fn enforces_page_order_size_and_snapshot_completeness() -> io::Result<()> {
    let delta = Header {
        commit: 5,
        ..delta_header()
    };
    let snapshot = Header {
        min_txid: 1,
        ..delta
    };
    for (header, numbers) in [
        (delta, vec![0]),
        (delta, vec![6]),
        (delta, vec![2, 2]),
        (delta, vec![3, 2]),
        (snapshot, vec![2]),
        (snapshot, vec![1, 3]),
    ] {
        let mut encoder = Encoder::new(Vec::new(), header)?;
        let mut result = Ok(());
        for number in numbers {
            result = encoder.write_page(number, &[1; 512]);
            if result.is_err() {
                break;
            }
        }
        assert!(result.is_err());
    }
    for size in [0, 511, 513] {
        assert!(
            Encoder::new(Vec::new(), delta)?
                .write_page(1, &vec![0; size])
                .is_err()
        );
    }
    let mut incomplete = Encoder::new(Vec::new(), snapshot)?;
    incomplete.write_page(1, &[1; 512])?;
    assert!(incomplete.finish(0).is_err());
    assert!(Encoder::new(Vec::new(), snapshot)?.finish(0).is_err());
    let sparse = encode(delta, &[1, 3, 5], 0)?;
    assert_eq!(read_all(&sparse)?, vec![1, 3, 5]);
    let lock = 0x40000000 / delta.page_size + 1;
    let large = Header {
        commit: u32::MAX,
        ..delta
    };
    assert!(
        Encoder::new(Vec::new(), large)?
            .write_page(lock, &[0; 512])
            .is_err()
    );
    assert_eq!(
        read_all(&encode(large, &[lock - 1, lock + 1, u32::MAX], 0)?)?,
        vec![lock - 1, lock + 1, u32::MAX]
    );
    Ok(())
}

#[test]
fn validates_empty_databases_and_post_apply_checksums() -> io::Result<()> {
    for flags in [0, NO_CHECKSUM] {
        for min_txid in [1, 2] {
            let header = Header {
                flags,
                commit: 0,
                min_txid,
                pre_apply_checksum: if flags == 0 && min_txid > 1 {
                    CHECKSUM_FLAG | 3
                } else {
                    0
                },
                ..delta_header()
            };
            let checksum = if flags == 0 { CHECKSUM_FLAG } else { 0 };
            assert!(read_all(&encode(header, &[], checksum)?)?.is_empty());
            for invalid in [
                1,
                CHECKSUM_FLAG | 1,
                if flags == 0 { 0 } else { CHECKSUM_FLAG },
            ] {
                assert!(
                    Encoder::new(Vec::new(), header)?.finish(invalid).is_err(),
                    "accepted checksum {invalid:x} for {header:?}"
                );
                let mut corrupt = encode(header, &[], checksum)?;
                let offset = corrupt.len() - 16;
                corrupt[offset..offset + 8].copy_from_slice(&invalid.to_be_bytes());
                rehash(&mut corrupt);
                assert!(
                    read_all(&corrupt).is_err(),
                    "decoded invalid empty database checksum"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn rejects_malformed_pages_indexes_and_trailers() -> io::Result<()> {
    let valid = encode(delta_header(), &[1], 0)?;
    let index_size = u64::from_be_bytes(
        valid[valid.len() - 24..valid.len() - 16]
            .try_into()
            .unwrap(),
    );
    let index = valid.len() - 24 - index_size as usize;
    for (offset, replacement, message) in [
        (0, vec![b'X'], "magic"),
        (4, 1u32.to_be_bytes().to_vec(), "flags"),
        (8, 513u32.to_be_bytes().to_vec(), "page size"),
        (16, 0u64.to_be_bytes().to_vec(), "transaction"),
        (100, 2u32.to_be_bytes().to_vec(), "page order"),
        (104, 2u16.to_be_bytes().to_vec(), "page flags"),
        (106, 0u32.to_be_bytes().to_vec(), "compressed page size"),
        (106, u32::MAX.to_be_bytes().to_vec(), "compressed page size"),
        (index, vec![2], "index number"),
        (index + 1, vec![99], "index offset"),
        (index + 2, vec![1], "index length"),
        (index + 3, vec![1], "extra page index"),
        (
            valid.len() - 24,
            123u64.to_be_bytes().to_vec(),
            "index size",
        ),
        (
            valid.len() - 8,
            0u64.to_be_bytes().to_vec(),
            "file checksum",
        ),
    ] {
        let mut corrupt = valid.clone();
        corrupt[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let error = read_all(&corrupt).unwrap_err();
        assert!(
            error.to_string().contains(message),
            "offset {offset}: {error}, expected {message}"
        );
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(
        read_all(&trailing)
            .unwrap_err()
            .to_string()
            .contains("trailing bytes")
    );
    let mut overflow = valid.clone();
    overflow.splice(index..index + 1, [0xff; 10]);
    assert!(
        read_all(&overflow)
            .unwrap_err()
            .to_string()
            .contains("varint overflow")
    );
    let mut invalid_post = valid;
    let offset = invalid_post.len() - 16;
    invalid_post[offset..offset + 8].copy_from_slice(&CHECKSUM_FLAG.to_be_bytes());
    rehash(&mut invalid_post);
    assert!(
        read_all(&invalid_post)
            .unwrap_err()
            .to_string()
            .contains("post-apply checksum")
    );
    Ok(())
}

#[test]
fn rejects_bad_compression_and_incomplete_or_inconsistent_snapshots() -> io::Result<()> {
    let valid = encode(delta_header(), &[1], 0)?;
    for data in [lz4_flex::block::compress(&[7; 511]), vec![0, 0, 0]] {
        let mut bytes = valid[..106].to_vec();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend(data);
        assert!(Decoder::new(bytes.as_slice())?.next_page().is_err());
    }
    for size in [511, 513] {
        let mut frame = lz4_flex::frame::FrameEncoder::new(Vec::new());
        frame.write_all(&vec![7; size])?;
        let mut bytes = valid[..104].to_vec();
        bytes.extend_from_slice(&[0; 2]);
        bytes.extend(frame.finish()?);
        assert!(Decoder::new(bytes.as_slice())?.next_page().is_err());
    }
    let mut incomplete = valid;
    incomplete[12..16].copy_from_slice(&2u32.to_be_bytes());
    incomplete[16..24].copy_from_slice(&1u64.to_be_bytes());
    assert!(
        read_all(&incomplete)
            .unwrap_err()
            .to_string()
            .contains("incomplete snapshot")
    );
    let mut missing = encode(
        Header {
            commit: 3,
            ..delta_header()
        },
        &[1, 3],
        0,
    )?;
    missing[16..24].copy_from_slice(&1u64.to_be_bytes());
    assert!(
        read_all(&missing)
            .unwrap_err()
            .to_string()
            .contains("missing page")
    );
    let mut checksum = include_bytes!("fixtures/0-1.ltx").to_vec();
    let offset = checksum.len() - 16;
    checksum[offset + 7] ^= 1;
    rehash(&mut checksum);
    assert!(
        read_all(&checksum)
            .unwrap_err()
            .to_string()
            .contains("snapshot checksum mismatch")
    );
    let mut decoder = Decoder::new(include_bytes!("fixtures/0-1.ltx").as_slice())?;
    let mut encoder = Encoder::new(Vec::new(), *decoder.header())?;
    while let Some(page) = decoder.next_page()? {
        encoder.write_page(page.number, &page.data)?;
    }
    assert!(
        encoder
            .finish(decoder.trailer().unwrap().post_apply_checksum ^ 1)
            .is_err()
    );
    Ok(())
}

#[test]
fn rejects_incompatible_compaction_inputs_and_checksum_chains() -> io::Result<()> {
    let header = delta_header();
    let first = encode(header, &[1], 0)?;
    for next in [
        Header {
            min_txid: 4,
            max_txid: 4,
            ..header
        },
        Header {
            min_txid: 1,
            max_txid: 1,
            ..header
        },
        header,
        Header {
            page_size: 1024,
            min_txid: 3,
            max_txid: 3,
            ..header
        },
        Header {
            flags: 0,
            min_txid: 3,
            max_txid: 3,
            pre_apply_checksum: CHECKSUM_FLAG,
            ..header
        },
    ] {
        let post = if next.flags == NO_CHECKSUM {
            0
        } else {
            CHECKSUM_FLAG | 5
        };
        let next = encode(next, &[1], post)?;
        assert!(compact(vec![first.as_slice(), &next], Vec::new()).is_err());
    }
    let checked = Header {
        flags: 0,
        pre_apply_checksum: CHECKSUM_FLAG | 1,
        ..header
    };
    let first = encode(checked, &[1], CHECKSUM_FLAG | 2)?;
    let next = encode(
        Header {
            min_txid: 3,
            max_txid: 3,
            pre_apply_checksum: CHECKSUM_FLAG | 3,
            ..checked
        },
        &[1],
        CHECKSUM_FLAG | 4,
    )?;
    assert!(
        compact(vec![first.as_slice(), &next], Vec::new())
            .unwrap_err()
            .to_string()
            .contains("transaction checksum")
    );
    let last = encode(
        Header {
            min_txid: u64::MAX,
            max_txid: u64::MAX,
            ..header
        },
        &[1],
        0,
    )?;
    let mut output = Vec::new();
    assert_eq!(
        compact(vec![last.as_slice()], &mut output)?.max_txid,
        u64::MAX
    );
    assert!(compact(vec![last.as_slice(), &last], Vec::new()).is_err());
    assert_eq!(read_all(&output)?, vec![1]);
    Ok(())
}

fn delta_header() -> Header {
    Header {
        flags: NO_CHECKSUM,
        page_size: 512,
        commit: 1,
        min_txid: 2,
        max_txid: 2,
        ..Header::default()
    }
}

fn encode(header: Header, pages: &[u32], checksum: u64) -> io::Result<Vec<u8>> {
    let mut encoder = Encoder::new(Vec::new(), header)?;
    for &page in pages {
        encoder.write_page(page, &vec![7; header.page_size as usize])?;
    }
    encoder.finish(checksum)
}

fn read_all(bytes: &[u8]) -> io::Result<Vec<u32>> {
    let mut decoder = Decoder::new(bytes)?;
    assert_eq!(decoder.trailer(), None);
    let mut pages = Vec::new();
    while let Some(page) = decoder.next_page()? {
        pages.push(page.number);
    }
    assert!(decoder.trailer().is_some());
    assert!(decoder.next_page()?.is_none());
    Ok(pages)
}

// Recompute the file checksum after a logical mutation so validation cannot hide behind CRC failure.
fn rehash(bytes: &mut [u8]) {
    let crc = crc::Crc::<u64>::new(&crc::CRC_64_GO_ISO);
    let mut hash = crc.digest();
    let page_size = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    hash.update(&bytes[..100]);
    let mut offset = 100;
    while bytes[offset..offset + 6] != [0; 6] {
        let length =
            u32::from_be_bytes(bytes[offset + 6..offset + 10].try_into().unwrap()) as usize;
        hash.update(&bytes[offset..offset + 10]);
        offset += 10;
        let data = lz4_flex::block::decompress(&bytes[offset..offset + length], page_size).unwrap();
        hash.update(&data);
        offset += length;
    }
    let end = bytes.len() - 8;
    hash.update(&bytes[offset..end]);
    bytes[end..].copy_from_slice(&(hash.finalize() | CHECKSUM_FLAG).to_be_bytes());
}
