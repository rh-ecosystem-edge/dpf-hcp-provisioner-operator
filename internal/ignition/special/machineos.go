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

package special

import (
	"encoding/json"
	"fmt"
	"net/url"
	"strings"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

// ReplaceMachineOSURL modifies the machine OS URL in the ignition config
func ReplaceMachineOSURL(ign *ignition.Ignition, machineOSURL string) error {
	// Find the machine config encapsulated file
	for i := range ign.Storage.Files {
		file := &ign.Storage.Files[i]
		if file.Path == "/etc/ignition-machine-config-encapsulated.json" {
			fmt.Println("Replacing machine OS URL")

			// Parse the data URI
			if !strings.HasPrefix(file.Contents.Source, "data:,") {
				return fmt.Errorf("unexpected data URI format")
			}

			// URL decode the content
			encodedData := strings.TrimPrefix(file.Contents.Source, "data:,")
			decodedText, err := url.QueryUnescape(encodedData)
			if err != nil {
				return fmt.Errorf("failed to decode data URI: %w", err)
			}

			// Parse JSON
			var config map[string]interface{}
			if err := json.Unmarshal([]byte(decodedText), &config); err != nil {
				return fmt.Errorf("failed to unmarshal machine config: %w", err)
			}

			// Modify the osImageURL
			spec, ok := config["spec"].(map[string]interface{})
			if !ok {
				return fmt.Errorf("spec not found in machine config")
			}
			spec["osImageURL"] = machineOSURL

			// Encode back to JSON
			modifiedJSON, err := json.Marshal(config)
			if err != nil {
				return fmt.Errorf("failed to marshal modified config: %w", err)
			}

			// URL encode and create data URI
			encodedText := url.QueryEscape(string(modifiedJSON))
			file.Contents.Source = fmt.Sprintf("data:,%s", encodedText)

			return nil
		}
	}

	return fmt.Errorf("machine OS URL file not found")
}
