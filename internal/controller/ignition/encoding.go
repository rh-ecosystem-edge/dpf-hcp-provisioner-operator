/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package ignition

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/url"
	"strings"
)

// NewEmptyIgnition creates a new empty ignition config with the given version.
func NewEmptyIgnition(version string) *Ignition {
	return &Ignition{
		Ignition: IgnitionConfig{Version: version},
	}
}

// EncodeIgnition serializes an Ignition config to JSON, then gzip-compresses and base64-encodes it.
func EncodeIgnition(ign *Ignition) (string, error) {
	data, err := json.Marshal(ign)
	if err != nil {
		return "", fmt.Errorf("marshaling ignition: %w", err)
	}

	var buf bytes.Buffer
	w := gzip.NewWriter(&buf)
	if _, err := w.Write(data); err != nil {
		return "", fmt.Errorf("gzip write: %w", err)
	}
	if err := w.Close(); err != nil {
		return "", fmt.Errorf("gzip close: %w", err)
	}

	return base64.StdEncoding.EncodeToString(buf.Bytes()), nil
}

// EncodeGzipFile gzip-compresses content and returns a FileContents with a data URI source.
func EncodeGzipFile(content []byte) FileContents {
	var buf bytes.Buffer
	w := gzip.NewWriter(&buf)
	_, _ = w.Write(content)
	_ = w.Close()
	encoded := base64.StdEncoding.EncodeToString(buf.Bytes())
	return FileContents{
		Source:      "data:;base64," + encoded,
		Compression: "gzip",
	}
}

// decodeGzipFile decodes a gzip+base64 encoded FileContents back to raw bytes.
func decodeGzipFile(fc FileContents) (data []byte, err error) {
	src := fc.Source
	const prefix = "data:;base64,"
	if !strings.HasPrefix(src, prefix) {
		return nil, fmt.Errorf("unexpected source prefix in FileContents: %q", src)
	}
	compressed, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(src, prefix))
	if err != nil {
		return nil, fmt.Errorf("base64 decode: %w", err)
	}
	r, err := gzip.NewReader(bytes.NewReader(compressed))
	if err != nil {
		return nil, fmt.Errorf("gzip reader: %w", err)
	}

	defer func() {
		if cerr := r.Close(); cerr != nil && err == nil {
			err = fmt.Errorf("gzip close: %w", cerr)
		}
	}()

	return io.ReadAll(r)
}

// DecodeIgnition decodes a gzip+base64 encoded ignition string back to an Ignition struct.
func DecodeIgnition(encoded string) (ign *Ignition, err error) {
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, fmt.Errorf("base64 decode: %w", err)
	}
	r, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("gzip reader: %w", err)
	}

	defer func() {
		if cerr := r.Close(); cerr != nil && err == nil {
			err = fmt.Errorf("gzip close: %w", cerr)
		}
	}()

	if err := json.NewDecoder(r).Decode(&ign); err != nil {
		return nil, fmt.Errorf("json decode: %w", err)
	}
	return ign, nil
}

// octalToDecimal converts an integer whose digits are in octal notation to a decimal integer.
// For example, octalToDecimal(755) = 493, octalToDecimal(644) = 420, octalToDecimal(600) = 384.
// This is needed when modes are written as human-readable octal digits (e.g. 755) rather than
// the actual decimal value (e.g. 493) that ignition expects.
func octalToDecimal(octal int) int {
	decimal := 0
	base := 1
	for octal > 0 {
		digit := octal % 10
		decimal += digit * base
		base *= 8
		octal /= 10
	}
	return decimal
}

// urlEncode encodes a string as a plain-text data URI suitable for use as an ignition file source.
func urlEncode(s string) string {
	return "data:text/plain," + url.QueryEscape(s)
}
