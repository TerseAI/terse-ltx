use crate::Header;

#[test]
fn snapshots_skip_the_sqlite_lock_page_and_end_at_commit() {
    for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        let lock = 0x40000000 / size + 1;
        let header = Header {
            page_size: size,
            commit: lock + 2,
            min_txid: 1,
            max_txid: 1,
            ..Header::default()
        };
        assert!(header.page(lock - 2, lock - 1).is_ok());
        assert!(header.page(lock - 1, lock + 1).is_ok());
        assert!(header.page(lock - 1, lock).is_err());
        assert!(header.page(lock - 1, lock + 2).is_err());
        assert!(header.complete(lock + 2).is_ok());
        assert!(header.complete(lock + 1).is_err());
        let ends_at_lock = Header {
            commit: lock,
            ..header
        };
        assert!(ends_at_lock.complete(lock - 1).is_ok());
        assert!(ends_at_lock.complete(lock).is_err());
    }
}
