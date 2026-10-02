use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};
use terse_ltx::{Decoder, Encoder, Header, Page, compact};

#[test]
fn compacts_go_history_with_updates_and_truncation() -> io::Result<()> {
    for flags in [0, 2] {
        for first in ["1", "frame"] {
            let history = history(flags, first);
            let mut output = Vec::new();
            let header = compact(history.iter().map(Vec::as_slice).collect(), &mut output)?;
            let expected = decode(&fixture(flags, "merged"))?;
            assert_eq!(header, expected.0);
            assert_eq!(decode(&output)?, expected);
            assert_eq!(header.min_txid, 1);
            assert_eq!(header.max_txid, 3);
            assert_eq!(header.commit, 3);
        }
    }
    Ok(())
}

#[test]
fn rejects_corruption_truncation_and_transaction_gaps() {
    for first in ["1", "frame"] {
        let valid = fixture(0, first);
        for length in 0..valid.len() {
            assert!(
                compact(vec![&valid[..length]], Vec::new()).is_err(),
                "accepted truncated file at {length}"
            );
        }
        let expected = decode(&valid).unwrap();
        for offset in 0..valid.len() {
            for bit in 0..8 {
                let mut damaged = valid.clone();
                damaged[offset] ^= 1 << bit;
                let mut output = Vec::new();
                if compact(vec![damaged.as_slice()], &mut output).is_ok() {
                    assert_eq!(
                        decode(&output).unwrap(),
                        expected,
                        "undetected corruption at byte {offset}, bit {bit}"
                    );
                }
            }
        }
    }
    let history = history(2, "1");
    for indices in [vec![0, 2], vec![1, 0], vec![0, 0], vec![]] {
        assert!(
            compact(
                indices.iter().map(|&i| history[i].as_slice()).collect(),
                Vec::new()
            )
            .is_err()
        );
    }
}

#[test]
#[ignore = "requires Go to verify Rust output with superfly/ltx v0.5.2"]
fn go_verifies_rust_compaction_and_encoding() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let oracle = directory.path().join("oracle");
    let status = Command::new("go")
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle"))
        .args(["build", "-mod=readonly", "-o"])
        .arg(&oracle)
        .arg(".")
        .status()?;
    assert!(status.success());
    for flags in [0, 2] {
        for first in ["1", "frame"] {
            let history = history(flags, first);
            let mut output = Vec::new();
            compact(history.iter().map(Vec::as_slice).collect(), &mut output)?;
            let path = directory.path().join("rust.ltx");
            fs::write(&path, output)?;
            let result = Command::new(&oracle)
                .arg("verify")
                .arg(path)
                .arg(fixture_path(flags, "merged"))
                .output()?;
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    let corpus = directory.path().join("matrix");
    let generated = Command::new(&oracle).arg("matrix").arg(&corpus).output()?;
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let mut cases = 0;
    for group in fs::read_dir(&corpus)? {
        for case in fs::read_dir(group?.path())? {
            let case = case?.path();
            verify_case(&oracle, &directory.path().join("rust.ltx"), &case)?;
            cases += 1;
        }
    }
    assert_eq!(cases, 650);
    println!("verified {cases} generated histories and {cases} encoder round trips with Go");
    Ok(())
}

fn verify_case(oracle: &Path, actual: &Path, case: &Path) -> io::Result<()> {
    let mut paths = fs::read_dir(case)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.retain(|path| path.file_name().unwrap() != "expected.ltx");
    paths.sort();
    let history = paths.iter().map(fs::read).collect::<io::Result<Vec<_>>>()?;
    let mut output = Vec::new();
    compact(history.iter().map(Vec::as_slice).collect(), &mut output)
        .unwrap_or_else(|error| panic!("{}: {error}", case.display()));
    let expected = case.join("expected.ltx");
    assert_eq!(
        decode(&output)?,
        decode(&fs::read(&expected)?)?,
        "{}",
        case.display()
    );
    fs::write(actual, output)?;
    verify_go(oracle, actual, &expected)?;
    let mut decoder = Decoder::new(history[0].as_slice())?;
    let mut encoder = Encoder::new(Vec::new(), *decoder.header())?;
    while let Some(page) = decoder.next_page()? {
        encoder.write_page(page.number, &page.data)?;
    }
    fs::write(
        actual,
        encoder.finish(decoder.trailer().unwrap().post_apply_checksum)?,
    )?;
    verify_go(oracle, actual, &paths[0])
}

fn history(flags: u32, first: &str) -> Vec<Vec<u8>> {
    [first, "2", "3"].map(|name| fixture(flags, name)).into()
}

fn fixture(flags: u32, name: &str) -> Vec<u8> {
    fs::read(fixture_path(flags, name)).unwrap()
}

fn fixture_path(flags: u32, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{flags}-{name}.ltx"))
}

fn decode(bytes: &[u8]) -> io::Result<(Header, Vec<Page>)> {
    let mut reader = Decoder::new(bytes)?;
    let header = *reader.header();
    let mut pages = Vec::new();
    while let Some(page) = reader.next_page()? {
        pages.push(page);
    }
    Ok((header, pages))
}

fn verify_go(oracle: &Path, actual: &Path, expected: &Path) -> io::Result<()> {
    let result = Command::new(oracle)
        .arg("verify")
        .arg(actual)
        .arg(expected)
        .output()?;
    assert!(
        result.status.success(),
        "{}: {}",
        expected.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
