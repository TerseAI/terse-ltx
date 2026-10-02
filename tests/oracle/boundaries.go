package main

import (
	"bytes"
	"fmt"
	"math"
	"os"
	"path/filepath"

	"github.com/superfly/ltx"
)

func boundaries(dir string) error {
	for size := uint32(512); size <= 65536; size *= 2 {
		var inputs [][]byte
		for i := uint64(0); i < 2; i++ {
			header := ltx.Header{Version: ltx.Version, Flags: ltx.HeaderFlagNoChecksum, PageSize: size, Commit: math.MaxUint32,
				MinTXID: ltx.TXID(math.MaxUint64 - 1 + i), MaxTXID: ltx.TXID(math.MaxUint64 - 1 + i), Timestamp: math.MinInt64,
				WALOffset: math.MaxInt64, WALSize: math.MaxInt64, WALSalt1: math.MaxUint32, WALSalt2: math.MaxUint32, NodeID: math.MaxUint64}
			if i == 1 {
				header.Timestamp = math.MaxInt64
			}
			var output bytes.Buffer
			encoder, err := ltx.NewEncoder(&output)
			if err != nil {
				return err
			}
			if err := encoder.EncodeHeader(header); err != nil {
				return err
			}
			for _, number := range []uint32{ltx.LockPgno(size) - 1, ltx.LockPgno(size) + 1, math.MaxUint32} {
				if err := encoder.EncodePage(ltx.PageHeader{Pgno: number}, matrixPage(size, number+uint32(i))); err != nil {
					return err
				}
			}
			if err := encoder.Close(); err != nil {
				return err
			}
			inputs = append(inputs, output.Bytes())
		}
		if err := writeCase(filepath.Join(dir, fmt.Sprint(size)), inputs, ltx.HeaderFlagNoChecksum); err != nil {
			return err
		}
	}
	var output bytes.Buffer
	encoder, err := ltx.NewEncoder(&output)
	if err != nil {
		return err
	}
	if err := encoder.EncodeHeader(ltx.Header{Version: ltx.Version, PageSize: 512, Commit: 5, MinTXID: 2, MaxTXID: 3, PreApplyChecksum: ltx.ChecksumFlag | 1}); err != nil {
		return err
	}
	encoder.SetPostApplyChecksum(ltx.ChecksumFlag | 1)
	if err := encoder.Close(); err != nil {
		return err
	}
	if err := writeCase(filepath.Join(dir, "no-op"), [][]byte{output.Bytes()}, 0); err != nil {
		return err
	}

	// Go's encoder rejects this combination, but its decoder accepts the format's zero checksum.
	empty, err := frameBytes(ltx.Header{Version: ltx.Version, Flags: ltx.HeaderFlagNoChecksum, PageSize: 512, MinTXID: 1, MaxTXID: 1}, nil, nil, 0)
	if err != nil {
		return err
	}
	path := filepath.Join(dir, "empty-unchecked")
	if err := os.MkdirAll(path, 0755); err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(path, "00.ltx"), empty, 0644); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(path, "expected.ltx"), empty, 0644)
}
