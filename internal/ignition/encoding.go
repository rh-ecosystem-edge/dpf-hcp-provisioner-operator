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
)

// NewEmptyIgnition creates a new empty ignition configuration
func NewEmptyIgnition(version string) *Ignition {
	if version == "" {
		version = "3.4.0"
	}
	return &Ignition{
		Ignition: IgnitionConfig{
			Version: version,
			Config:  make(map[string]interface{}),
		},
		Storage: StorageFiles{
			Files: []FileEntry{},
		},
		Systemd: SystemdUnits{
			Units: []SystemdUnit{},
		},
		KernelArguments: &KernelArguments{
			ShouldExist:    []string{},
			ShouldNotExist: []string{},
		},
	}
}

// EncodeIgnition encodes an ignition config to gzip + base64
func EncodeIgnition(ign *Ignition) (string, error) {
	// Marshal to JSON with compact separators (no spaces)
	jsonBytes, err := json.Marshal(ign)
	if err != nil {
		return "", fmt.Errorf("failed to marshal ignition: %w", err)
	}

	// Gzip compress
	var buf bytes.Buffer
	gzWriter := gzip.NewWriter(&buf)
	if _, err := gzWriter.Write(jsonBytes); err != nil {
		return "", fmt.Errorf("failed to gzip ignition: %w", err)
	}
	if err := gzWriter.Close(); err != nil {
		return "", fmt.Errorf("failed to close gzip writer: %w", err)
	}

	// Base64 encode
	encoded := base64.StdEncoding.EncodeToString(buf.Bytes())
	return encoded, nil
}

// EncodeGzipFile encodes file content with gzip compression and base64 encoding
func EncodeGzipFile(content []byte) (FileContents, error) {
	// Gzip compress
	var buf bytes.Buffer
	gzWriter := gzip.NewWriter(&buf)
	if _, err := gzWriter.Write(content); err != nil {
		return FileContents{}, fmt.Errorf("failed to gzip content: %w", err)
	}
	if err := gzWriter.Close(); err != nil {
		return FileContents{}, fmt.Errorf("failed to close gzip writer: %w", err)
	}

	// Base64 encode and create data URI
	encoded := base64.StdEncoding.EncodeToString(buf.Bytes())
	return FileContents{
		Compression: "gzip",
		Source:      fmt.Sprintf("data:;base64,%s", encoded),
	}, nil
}

// DecodeIgnition decodes a base64 + gzip encoded ignition config
func DecodeIgnition(encoded string) (*Ignition, error) {
	// Base64 decode
	compressed, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, fmt.Errorf("failed to base64 decode: %w", err)
	}

	// Gunzip
	gzReader, err := gzip.NewReader(bytes.NewReader(compressed))
	if err != nil {
		return nil, fmt.Errorf("failed to create gzip reader: %w", err)
	}
	defer func() { _ = gzReader.Close() }()

	var buf bytes.Buffer
	if _, err := buf.ReadFrom(gzReader); err != nil {
		return nil, fmt.Errorf("failed to decompress: %w", err)
	}

	// Unmarshal JSON
	var ign Ignition
	if err := json.Unmarshal(buf.Bytes(), &ign); err != nil {
		return nil, fmt.Errorf("failed to unmarshal ignition: %w", err)
	}

	return &ign, nil
}
