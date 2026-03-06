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
	"fmt"
	"net/url"
	"strings"

	"encoding/json"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
)

// ReplaceMachineOSURL modifies the machine OS URL in the ignition config
func ReplaceMachineOSURL(ign *igntypes.Config, machineOSURL string) error {
	for i := range ign.Storage.Files {
		file := &ign.Storage.Files[i]
		if file.Path == "/etc/ignition-machine-config-encapsulated.json" {
			fmt.Println("Replacing machine OS URL")

			if file.Contents.Source == nil || !strings.HasPrefix(*file.Contents.Source, "data:,") {
				return fmt.Errorf("unexpected data URI format")
			}

			encodedData := strings.TrimPrefix(*file.Contents.Source, "data:,")
			decodedText, err := url.QueryUnescape(encodedData)
			if err != nil {
				return fmt.Errorf("failed to decode data URI: %w", err)
			}

			var config map[string]interface{}
			if err := json.Unmarshal([]byte(decodedText), &config); err != nil {
				return fmt.Errorf("failed to unmarshal machine config: %w", err)
			}

			spec, ok := config["spec"].(map[string]interface{})
			if !ok {
				return fmt.Errorf("spec not found in machine config")
			}
			spec["osImageURL"] = machineOSURL

			modifiedJSON, err := json.Marshal(config)
			if err != nil {
				return fmt.Errorf("failed to marshal modified config: %w", err)
			}

			encodedText := url.QueryEscape(string(modifiedJSON))
			newSource := fmt.Sprintf("data:,%s", encodedText)
			file.Contents.Source = &newSource

			return nil
		}
	}

	return fmt.Errorf("machine OS URL file not found")
}
