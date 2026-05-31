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
	"encoding/json"
	"fmt"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	ignvalidate "github.com/coreos/ignition/v2/config/validate"
)

// ValidateConfig validates an ignition config using the same logic as ignition-validate.
// It marshals the config to JSON and runs full validation including duplicate entry detection.
func ValidateConfig(cfg *igntypes.Config) error {
	raw, err := json.Marshal(cfg)
	if err != nil {
		return fmt.Errorf("failed to marshal ignition config for validation: %w", err)
	}

	rpt := ignvalidate.ValidateWithContext(*cfg, raw)
	if rpt.IsFatal() {
		return fmt.Errorf("ignition config is invalid: %s", rpt.String())
	}
	return nil
}
