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
	"fmt"

	"encoding/json"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
)

// NewEmptyIgnition creates a new empty ignition configuration
func NewEmptyIgnition(version string) *igntypes.Config {
	if version == "" {
		version = "3.4.0"
	}
	return &igntypes.Config{
		Ignition: igntypes.Ignition{
			Version: version,
		},
	}
}

// MarshalJSON marshals an ignition config to JSON, stripping empty objects
// that the official types produce due to Go's omitempty not handling zero-value structs.
func MarshalJSON(ign *igntypes.Config) ([]byte, error) {
	jsonBytes, err := json.Marshal(ign)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal ignition: %w", err)
	}
	return stripEmptyObjects(jsonBytes)
}

// stripEmptyObjects removes empty JSON objects ({}) recursively from JSON bytes.
func stripEmptyObjects(data []byte) ([]byte, error) {
	var m map[string]any
	if err := json.Unmarshal(data, &m); err != nil {
		return nil, err
	}
	stripEmpty(m)
	return json.Marshal(m)
}

func stripEmpty(m map[string]any) {
	for k, v := range m {
		switch val := v.(type) {
		case map[string]any:
			stripEmpty(val)
			if len(val) == 0 {
				delete(m, k)
			}
		case []any:
			for _, item := range val {
				if nested, ok := item.(map[string]any); ok {
					stripEmpty(nested)
				}
			}
		}
	}
}

// EncodeIgnition encodes an ignition config to gzip + base64
func EncodeIgnition(ign *igntypes.Config) (string, error) {
	jsonBytes, err := MarshalJSON(ign)
	if err != nil {
		return "", fmt.Errorf("failed to marshal ignition: %w", err)
	}

	var buf bytes.Buffer
	gzWriter, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	if _, err := gzWriter.Write(jsonBytes); err != nil {
		return "", fmt.Errorf("failed to gzip ignition: %w", err)
	}
	if err := gzWriter.Close(); err != nil {
		return "", fmt.Errorf("failed to close gzip writer: %w", err)
	}

	encoded := base64.StdEncoding.EncodeToString(buf.Bytes())
	return encoded, nil
}

// EncodeGzipFile encodes file content with gzip compression and base64 encoding
func EncodeGzipFile(content []byte) (igntypes.Resource, error) {
	var buf bytes.Buffer
	gzWriter, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	if _, err := gzWriter.Write(content); err != nil {
		return igntypes.Resource{}, fmt.Errorf("failed to gzip content: %w", err)
	}
	if err := gzWriter.Close(); err != nil {
		return igntypes.Resource{}, fmt.Errorf("failed to close gzip writer: %w", err)
	}

	source := fmt.Sprintf("data:;base64,%s", base64.StdEncoding.EncodeToString(buf.Bytes()))
	compression := "gzip"
	return igntypes.Resource{
		Source:      &source,
		Compression: &compression,
	}, nil
}

// DecodeIgnition decodes a base64 + gzip encoded ignition config
func DecodeIgnition(encoded string) (*igntypes.Config, error) {
	compressed, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, fmt.Errorf("failed to base64 decode: %w", err)
	}

	gzReader, err := gzip.NewReader(bytes.NewReader(compressed))
	if err != nil {
		return nil, fmt.Errorf("failed to create gzip reader: %w", err)
	}
	defer gzReader.Close()

	var buf bytes.Buffer
	if _, err := buf.ReadFrom(gzReader); err != nil {
		return nil, fmt.Errorf("failed to decompress: %w", err)
	}

	var ign igntypes.Config
	if err := json.Unmarshal(buf.Bytes(), &ign); err != nil {
		return nil, fmt.Errorf("failed to unmarshal ignition: %w", err)
	}

	return &ign, nil
}
