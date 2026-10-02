package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/superfly/ltx"
)

func matrix(dir string) error {
	for size := uint32(512); size <= 65536; size *= 2 {
		for _, flags := range []uint32{0, ltx.HeaderFlagNoChecksum} {
			for seed := 0; seed < 8; seed++ {
				name := filepath.Join(dir, fmt.Sprintf("%d-%d-%d", size, flags, seed))
				inputs, err := matrixHistory(size, flags, seed)
				if err != nil {
					return fmt.Errorf("%s: %w", name, err)
				}
				if err := writeCase(filepath.Join(name, "full"), inputs, flags); err != nil {
					return err
				}
				if err := writeCase(filepath.Join(name, "incremental"), inputs[1:], flags); err != nil {
					return err
				}
				if err := writeCase(filepath.Join(name, "single"), inputs[len(inputs)-1:], flags); err != nil {
					return err
				}
				checkpoint, err := merge(inputs[:4], flags)
				if err != nil {
					return err
				}
				if err := writeCase(filepath.Join(name, "checkpoint-tail"), append([][]byte{checkpoint}, inputs[4:]...), flags); err != nil {
					return err
				}

				left, err := merge(inputs[:5], flags)
				if err != nil {
					return err
				}
				right, err := merge(inputs[3:], flags)
				if err != nil {
					return err
				}
				if err := writeCase(filepath.Join(name, "overlap"), [][]byte{left, right}, flags); err != nil {
					return err
				}
			}
		}
	}
	return boundaries(filepath.Join(dir, "boundaries"))
}

func matrixHistory(size, flags uint32, seed int) ([][]byte, error) {
	commits := []int{0, 4, 4, 7, 3, 6, 0, 5}
	var pages [][]byte
	var inputs [][]byte
	var previous ltx.Checksum
	txid := ltx.TXID(1)
	for tx := range commits {
		count := commits[(tx+seed)%len(commits)]
		// Go v0.5.2 cannot encode empty databases with checksums disabled.
		if flags == ltx.HeaderFlagNoChecksum && count == 0 {
			count = 1
		}
		old := len(pages)
		if count < old {
			pages = pages[:count]
		}
		for len(pages) < count {
			pages = append(pages, nil)
		}
		var selected []uint32
		for i := range pages {
			if tx == 0 || i >= old || (i+tx+seed)%3 == 0 {
				pages[i] = matrixPage(size, uint32(seed*100+tx*10+i))
				selected = append(selected, uint32(i+1))
			}
		}
		checksum := ltx.Checksum(0)
		if flags == 0 {
			checksum = ltx.ChecksumFlag
			for i, data := range pages {
				checksum = ltx.ChecksumFlag | (checksum ^ ltx.ChecksumPage(uint32(i+1), data))
			}
		}
		header := ltx.Header{Version: ltx.Version, Flags: flags, PageSize: size, Commit: uint32(count), MinTXID: txid, MaxTXID: txid + ltx.TXID((tx+seed)%3), Timestamp: int64(tx*1000 - seed), PreApplyChecksum: previous,
			WALOffset: int64(32 + tx*(int(size)+24)), WALSize: int64(len(selected) * (int(size) + 24)), WALSalt1: 17, WALSalt2: 29, NodeID: 42}
		var buf bytes.Buffer
		encoder, err := ltx.NewEncoder(&buf)
		if err != nil {
			return nil, err
		}
		if err := encoder.EncodeHeader(header); err != nil {
			return nil, err
		}
		for _, pgno := range selected {
			if err := encoder.EncodePage(ltx.PageHeader{Pgno: pgno}, pages[pgno-1]); err != nil {
				return nil, err
			}
		}
		encoder.SetPostApplyChecksum(checksum)
		if err := encoder.Close(); err != nil {
			return nil, err
		}
		encoded := buf.Bytes()
		if (tx+seed)%2 == 1 {
			encoded, err = frameBytes(header, selected, pages, checksum)
			if err != nil {
				return nil, err
			}
		}
		inputs = append(inputs, encoded)
		previous, txid = checksum, header.MaxTXID+1
	}
	return inputs, nil
}

func matrixPage(size, seed uint32) []byte {
	data := make([]byte, size)
	pattern := seed % 3
	for i := range data {
		switch pattern {
		case 0:
			data[i] = byte(seed)
		case 1:
			data[i] = byte(i % 17)
		default:
			seed = seed*1664525 + 1013904223
			data[i] = byte(seed >> 24)
		}
	}
	return data
}

func writeCase(dir string, inputs [][]byte, flags uint32) error {
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}
	for i, input := range inputs {
		if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("%02d.ltx", i)), input, 0644); err != nil {
			return err
		}
	}
	output, err := merge(inputs, flags)
	if err != nil {
		return fmt.Errorf("%s: %w", dir, err)
	}
	return os.WriteFile(filepath.Join(dir, "expected.ltx"), output, 0644)
}

func merge(inputs [][]byte, flags uint32) ([]byte, error) {
	readers := make([]io.Reader, len(inputs))
	for i, data := range inputs {
		readers[i] = bytes.NewReader(data)
	}
	var output bytes.Buffer
	compactor, err := ltx.NewCompactor(&output, readers)
	if err != nil {
		return nil, err
	}
	compactor.HeaderFlags = flags
	if err := compactor.Compact(context.Background()); err != nil {
		return nil, err
	}
	return output.Bytes(), nil
}
