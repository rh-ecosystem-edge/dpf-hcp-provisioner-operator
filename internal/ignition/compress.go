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
	"net/url"
	"strings"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
)

// GzipIgnitionFiles gzip-compresses all uncompressed files in an ignition config's storage.
// Files that already have compression set are left as-is.
func GzipIgnitionFiles(ign *igntypes.Config) error {
	for i := range ign.Storage.Files {
		contents := &ign.Storage.Files[i].Contents

		if contents.Compression != nil && *contents.Compression != "" {
			continue
		}

		if contents.Source == nil || *contents.Source == "" {
			continue
		}

		source := *contents.Source

		var raw []byte
		switch {
		case strings.Contains(source, ";base64,"):
			parts := strings.SplitN(source, ";base64,", 2)
			decoded, err := base64.StdEncoding.DecodeString(parts[1])
			if err != nil {
				return fmt.Errorf("failed to base64-decode file source: %w", err)
			}
			raw = decoded
		case strings.HasPrefix(source, "data:"):
			idx := strings.Index(source, ",")
			if idx < 0 {
				continue
			}
			decoded, err := url.QueryUnescape(source[idx+1:])
			if err != nil {
				return fmt.Errorf("failed to URL-decode file source: %w", err)
			}
			raw = []byte(decoded)
		default:
			continue
		}

		var buf bytes.Buffer
		gzWriter, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
		if _, err := gzWriter.Write(raw); err != nil {
			return fmt.Errorf("failed to gzip file content: %w", err)
		}
		if err := gzWriter.Close(); err != nil {
			return fmt.Errorf("failed to close gzip writer: %w", err)
		}

		newSource := fmt.Sprintf("data:;base64,%s", base64.StdEncoding.EncodeToString(buf.Bytes()))
		compression := "gzip"
		contents.Source = &newSource
		contents.Compression = &compression
	}

	return nil
}
