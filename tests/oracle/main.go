package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"fmt"
	"hash/crc64"
	"io"
	"os"
	"path/filepath"

	"github.com/pierrec/lz4/v4"
	"github.com/superfly/ltx"
)

func main() {
	var err error
	switch os.Args[1] {
	case "generate":
		err = generate(os.Args[2])
	case "matrix":
		err = matrix(os.Args[2])
	case "verify":
		err = verify(os.Args[2], os.Args[3])
	default:
		err = fmt.Errorf("unknown command")
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func generate(dir string) error {
	for _, flags := range []uint32{0, ltx.HeaderFlagNoChecksum} {
		var readers []io.Reader
		pages := [][]byte{page(1), page(2), page(3), page(4)}
		previous := ltx.Checksum(0)
		for tx := 1; tx <= 3; tx++ {
			selected := []uint32{1, 2, 3, 4}
			if tx == 2 {
				pages[1] = page(12)
				pages[3] = page(14)
				selected = []uint32{2, 4}
			}
			if tx == 3 {
				pages = pages[:3]
				pages[2] = page(23)
				selected = []uint32{3}
			}
			var checksum ltx.Checksum
			if flags == 0 {
				checksum = ltx.ChecksumFlag
				for i, data := range pages {
					checksum = ltx.ChecksumFlag | (checksum ^ ltx.ChecksumPage(uint32(i+1), data))
				}
			}
			header := ltx.Header{Version: ltx.Version, Flags: flags, PageSize: 512, Commit: uint32(len(pages)), MinTXID: ltx.TXID(tx), MaxTXID: ltx.TXID(tx), Timestamp: int64(tx * 1000), PreApplyChecksum: previous}
			var buf bytes.Buffer
			enc, _ := ltx.NewEncoder(&buf)
			if err := enc.EncodeHeader(header); err != nil {
				return err
			}
			for _, pgno := range selected {
				if err := enc.EncodePage(ltx.PageHeader{Pgno: pgno}, pages[pgno-1]); err != nil {
					return err
				}
			}
			enc.SetPostApplyChecksum(checksum)
			if err := enc.Close(); err != nil {
				return err
			}
			name := fmt.Sprintf("%d-%d.ltx", flags, tx)
			if err := os.WriteFile(filepath.Join(dir, name), buf.Bytes(), 0644); err != nil {
				return err
			}
			readers = append(readers, bytes.NewReader(buf.Bytes()))
			if tx == 1 {
				if err := writeFrame(filepath.Join(dir, fmt.Sprintf("%d-frame.ltx", flags)), header, pages, checksum); err != nil {
					return err
				}
			}
			previous = checksum
		}
		var merged bytes.Buffer
		compactor, _ := ltx.NewCompactor(&merged, readers)
		compactor.HeaderFlags = flags
		if err := compactor.Compact(context.Background()); err != nil {
			return err
		}
		if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("%d-merged.ltx", flags)), merged.Bytes(), 0644); err != nil {
			return err
		}
	}
	return nil
}

func page(seed uint32) []byte {
	data := make([]byte, 512)
	for i := range data {
		seed = seed*1664525 + 1013904223
		data[i] = byte(seed >> 24)
	}
	return data
}

// Exercise the older per-page frame layout with the same Go header and checksum rules.
func writeFrame(path string, header ltx.Header, pages [][]byte, checksum ltx.Checksum) error {
	selected := make([]uint32, len(pages))
	for i := range pages {
		selected[i] = uint32(i + 1)
	}
	data, err := frameBytes(header, selected, pages, checksum)
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0644)
}

func frameBytes(header ltx.Header, selected []uint32, pages [][]byte, checksum ltx.Checksum) ([]byte, error) {
	var file bytes.Buffer
	hash := crc64.New(crc64.MakeTable(crc64.ISO))
	write := func(data []byte) { file.Write(data); hash.Write(data) }
	raw, err := header.MarshalBinary()
	if err != nil {
		return nil, err
	}
	write(raw)
	var index []byte
	for _, number := range selected {
		data := pages[number-1]
		offset := file.Len()
		raw, err := (&ltx.PageHeader{Pgno: number}).MarshalBinary()
		if err != nil {
			return nil, err
		}
		write(raw)
		encoder := lz4.NewWriter(&file)
		if err := encoder.Apply(lz4.BlockSizeOption(lz4.Block64Kb), lz4.ChecksumOption(true)); err != nil {
			return nil, err
		}
		if _, err := encoder.Write(data); err != nil {
			return nil, err
		}
		if err := encoder.Close(); err != nil {
			return nil, err
		}
		hash.Write(data)
		index = binary.AppendUvarint(index, uint64(number))
		index = binary.AppendUvarint(index, uint64(offset))
		index = binary.AppendUvarint(index, uint64(file.Len()-offset))
	}
	write(make([]byte, ltx.PageHeaderSize))
	index = binary.AppendUvarint(index, 0)
	write(index)
	write(binary.BigEndian.AppendUint64(nil, uint64(len(index))))
	write(binary.BigEndian.AppendUint64(nil, uint64(checksum)))
	file.Write(binary.BigEndian.AppendUint64(nil, uint64(ltx.ChecksumFlag)|hash.Sum64()))
	if err := ltx.NewDecoder(bytes.NewReader(file.Bytes())).Verify(); err != nil {
		return nil, err
	}
	return file.Bytes(), nil
}

func verify(actual, expected string) error {
	a, err := os.Open(actual)
	if err != nil {
		return err
	}
	defer a.Close()
	b, err := os.Open(expected)
	if err != nil {
		return err
	}
	defer b.Close()
	ad, bd := ltx.NewDecoder(a), ltx.NewDecoder(b)
	if err := ad.DecodeHeader(); err != nil {
		return err
	}
	if err := bd.DecodeHeader(); err != nil {
		return err
	}
	if ad.Header() != bd.Header() {
		return fmt.Errorf("header differs from Go output")
	}
	actualPage, expectedPage := make([]byte, ad.Header().PageSize), make([]byte, bd.Header().PageSize)
	for {
		var ah, bh ltx.PageHeader
		ae, be := ad.DecodePage(&ah, actualPage), bd.DecodePage(&bh, expectedPage)
		if ae == io.EOF && be == io.EOF {
			break
		}
		if ae != nil || be != nil {
			return fmt.Errorf("page decoding differs: %v / %v", ae, be)
		}
		if ah.Pgno != bh.Pgno || !bytes.Equal(actualPage, expectedPage) {
			return fmt.Errorf("page differs from Go output")
		}
	}
	if err := ad.Close(); err != nil {
		return err
	}
	if err := bd.Close(); err != nil {
		return err
	}
	if ad.Trailer().PostApplyChecksum != bd.Trailer().PostApplyChecksum {
		return fmt.Errorf("post-apply checksum differs")
	}
	return nil
}
